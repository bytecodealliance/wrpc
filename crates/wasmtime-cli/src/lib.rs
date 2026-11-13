use std::sync::Arc;

use anyhow::{Context as _, anyhow, bail};
use clap::Parser;
use futures::{Stream, StreamExt as _};
use tokio::fs;
use tokio::task::JoinSet;
use tracing::{Instrument as _, Span, error, info, instrument, warn};
use url::Url;
use wasi_preview1_component_adapter_provider::{
    WASI_SNAPSHOT_PREVIEW1_ADAPTER_NAME, WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER,
};
use wasmtime::component::{Component, InstancePre, Linker, ResourceTable, ResourceType, types};
use wasmtime::{Config, Engine, Memory, MemoryType, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpCtxView, WasiHttpView};
use wrpc_transport::Serve;
use wrpc_wasmtime::{
    ResourceTypes, ServeExt as _, SharedResourceTable, SharedStore, WrpcCtxView, WrpcView,
    collect_component_resource_exports,
};

mod tcp;

const DEFAULT_TIMEOUT: &str = "10s";

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
enum Command {
    #[command(subcommand)]
    Tcp(tcp::Command),
}

pub enum Workload {
    Url(Url),
    Binary(Vec<u8>),
}

pub struct Ctx {
    pub table: ResourceTable,
    pub wasi: WasiCtx,
    pub http: WasiHttpCtx,
    pub shared_resources: SharedResourceTable,
}

impl WrpcView for Ctx {
    fn wrpc(&mut self) -> WrpcCtxView<'_> {
        WrpcCtxView {
            shared_resources: &mut self.shared_resources,
            table: &mut self.table,
        }
    }
}

impl WasiView for Ctx {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl WasiHttpView for Ctx {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView {
            ctx: &mut self.http,
            table: &mut self.table,
            hooks: wasmtime_wasi_http::default_hooks(),
        }
    }
}

// https://github.com/bytecodealliance/wasmtime/blob/b943666650696f1eb7ff8b217762b58d5ef5779d/src/commands/serve.rs#L641-L656
fn use_pooling_allocator_by_default() -> anyhow::Result<Option<bool>> {
    const BITS_TO_TEST: u32 = 42;
    let mut config = Config::default();
    config.wasm_memory64(true);
    config.memory_reservation(1 << BITS_TO_TEST);
    let engine = Engine::new(&config)?;
    let mut store = Store::new(&engine, ());
    // NB: the maximum size is in wasm pages to take out the 16-bits of wasm
    // page size here from the maximum size.
    let ty = MemoryType::new64(0, Some(1 << (BITS_TO_TEST - 16)))?;
    if Memory::new(&mut store, ty).is_ok() {
        Ok(Some(true))
    } else {
        Ok(None)
    }
}

#[instrument(level = "trace")]
async fn instantiate_pre(
    workload: &str,
) -> anyhow::Result<(InstancePre<Ctx>, Engine, Arc<[ResourceType]>)> {
    let mut opts = wasmtime_cli_flags::CommonOptions::new();
    let mut config = opts
        .config(use_pooling_allocator_by_default().unwrap_or(None))
        .map_err(anyhow::Error::from)
        .context("failed to construct Wasmtime config")?;
    config.wasm_component_model(true);
    let engine = Engine::new(&config)
        .map_err(anyhow::Error::from)
        .context("failed to initialize Wasmtime engine")?;

    let wasm = if workload.starts_with('.') || workload.starts_with('/') {
        fs::read(&workload)
            .await
            .with_context(|| format!("failed to read relative path to workload `{workload}`"))
            .map(Workload::Binary)
    } else {
        Url::parse(workload)
            .with_context(|| format!("failed to parse Wasm URL `{workload}`"))
            .map(Workload::Url)
    }?;
    let wasm = match wasm {
        Workload::Url(wasm) => match wasm.scheme() {
            "file" => {
                let wasm = wasm
                    .to_file_path()
                    .map_err(|()| anyhow!("failed to convert Wasm URL to file path"))?;
                fs::read(wasm)
                    .await
                    .context("failed to read Wasm from file URL")?
            }
            "http" | "https" => {
                let wasm = reqwest::get(wasm).await.context("failed to GET Wasm URL")?;
                let wasm = wasm.bytes().await.context("failed fetch Wasm from URL")?;
                wasm.to_vec()
            }
            scheme => bail!("URL scheme `{scheme}` not supported"),
        },
        Workload::Binary(wasm) => wasm,
    };
    let wasm = if wasmparser::Parser::is_core_wasm(&wasm) {
        wit_component::ComponentEncoder::default()
            .validate(true)
            .module(&wasm)
            .context("failed to set core component module")?
            .adapter(
                WASI_SNAPSHOT_PREVIEW1_ADAPTER_NAME,
                WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER,
            )
            .context("failed to add WASI adapter")?
            .encode()
            .context("failed to encode a component")?
    } else {
        wasm
    };

    let component = Component::new(&engine, wasm)
        .map_err(anyhow::Error::from)
        .context("failed to compile component")?;

    let mut linker = Linker::<Ctx>::new(&engine);
    wasmtime_wasi::p2::add_to_linker_async(&mut linker)
        .map_err(anyhow::Error::from)
        .context("failed to link WASI 0.2")?;
    wasmtime_wasi::p3::add_to_linker(&mut linker)
        .map_err(anyhow::Error::from)
        .context("failed to link WASI 0.3")?;
    wasmtime_wasi_http::p2::add_only_http_to_linker_async(&mut linker)
        .map_err(anyhow::Error::from)
        .context("failed to link `wasi:http` 0.2")?;
    wasmtime_wasi_http::p3::add_to_linker(&mut linker)
        .map_err(anyhow::Error::from)
        .context("failed to link `wasi:http` 0.3")?;
    let mut guest_resources = Vec::new();
    collect_component_resource_exports(&engine, &component.component_type(), &mut guest_resources);

    let pre = linker
        .instantiate_pre(&component)
        .map_err(anyhow::Error::from)
        .context("failed to pre-instantiate component")?;
    Ok((pre, engine, Arc::from(guest_resources)))
}

fn new_store(
    engine: &Engine,
    envs: impl IntoIterator<Item = (impl AsRef<str>, impl AsRef<str>)>,
) -> Store<Ctx> {
    let mut wasi = WasiCtxBuilder::new();
    wasi.inherit_env()
        .inherit_stdio()
        .inherit_network()
        .allow_ip_name_lookup(true)
        .allow_tcp(true)
        .allow_udp(true)
        .args(&["reactor.wasm"]);
    for (k, v) in envs {
        wasi.env(k, v);
    }
    Store::new(
        engine,
        Ctx {
            wasi: wasi.build(),
            http: WasiHttpCtx::new(),
            table: ResourceTable::new(),
            shared_resources: SharedResourceTable::default(),
        },
    )
}

async fn handle_invocations<T>(
    invocations: impl Stream<Item = anyhow::Result<(T, impl Future<Output = anyhow::Result<()>>)>>,
    kind: &'static str,
) {
    invocations
        .for_each_concurrent(None, |invocation| async move {
            match invocation {
                Ok((_, fut)) => {
                    info!(kind, "serving function invocation");
                    if let Err(err) = fut.await {
                        warn!(?err, kind, "failed to serve function invocation");
                    } else {
                        info!(kind, "successfully served function invocation");
                    }
                }
                Err(err) => {
                    error!(?err, kind, "failed to accept function invocation");
                }
            }
        })
        .await;
}

#[instrument(level = "trace", skip_all, ret(level = "trace"))]
pub async fn serve_shared<S: Serve>(
    handlers: &mut JoinSet<anyhow::Result<()>>,
    srv: S,
    mut store: Store<Ctx>,
    pre: InstancePre<Ctx>,
    guest_resources: Arc<[ResourceType]>,
) -> anyhow::Result<()> {
    let span = Span::current();
    let instance = pre
        .instantiate_async(&mut store)
        .await
        .map_err(anyhow::Error::from)
        .context("failed to instantiate component")?;
    let engine = store.engine().clone();
    let resources = ResourceTypes {
        guest: guest_resources,
        io_streams: wrpc_wasmtime::paths::wasi_io_stream_resources(
            &engine,
            &pre.component().component_type(),
        )
        .into(),
    };
    let (store, event_loop) = SharedStore::new(store);
    handlers.spawn(event_loop.instrument(span.clone()));
    for (name, types::ComponentExtern { ty, .. }) in
        pre.component().component_type().exports(&engine)
    {
        match (name, ty) {
            (name, types::ComponentItem::ComponentFunc(ty)) => {
                info!(?name, "serving root function");
                let idx = pre
                    .component()
                    .get_export_index(None, name)
                    .with_context(|| format!("export `{name}` not found"))?;
                let invocations = srv
                    .serve_function_shared(
                        store.clone(),
                        instance,
                        idx,
                        resources.clone(),
                        ty,
                        "",
                        name,
                    )
                    .await?;
                handlers.spawn(
                    async move {
                        handle_invocations(invocations, "root").await;
                        Ok(())
                    }
                    .instrument(span.clone()),
                );
            }
            (_, types::ComponentItem::CoreFunc(_)) => {
                warn!(name, "serving root core function exports not supported yet");
            }
            (_, types::ComponentItem::Module(_)) => {
                warn!(name, "serving root module exports not supported yet");
            }
            (_, types::ComponentItem::Component(_)) => {
                warn!(name, "serving root component exports not supported yet");
            }
            (instance_name, types::ComponentItem::ComponentInstance(ty)) => {
                let instance_idx = pre
                    .component()
                    .get_export_index(None, instance_name)
                    .with_context(|| format!("export `{instance_name}` not found"))?;
                for (name, types::ComponentExtern { ty, .. }) in ty.exports(&engine) {
                    match ty {
                        types::ComponentItem::ComponentFunc(ty) => {
                            info!(?name, "serving instance function");
                            let idx = pre
                                .component()
                                .get_export_index(Some(&instance_idx), name)
                                .with_context(|| format!("export `{name}` not found"))?;
                            let invocations = srv
                                .serve_function_shared(
                                    store.clone(),
                                    instance,
                                    idx,
                                    resources.clone(),
                                    ty,
                                    instance_name,
                                    name,
                                )
                                .await?;
                            handlers.spawn(
                                async move {
                                    handle_invocations(invocations, "instance").await;
                                    Ok(())
                                }
                                .instrument(span.clone()),
                            );
                        }
                        types::ComponentItem::CoreFunc(_) => {
                            warn!(
                                instance_name,
                                name, "serving instance core function exports not supported yet"
                            );
                        }
                        types::ComponentItem::Module(_) => {
                            warn!(
                                instance_name,
                                name, "serving instance module exports not supported yet"
                            );
                        }
                        types::ComponentItem::Component(_) => {
                            warn!(
                                instance_name,
                                name, "serving instance component exports not supported yet"
                            );
                        }
                        types::ComponentItem::ComponentInstance(_) => {
                            warn!(
                                instance_name,
                                name, "serving nested instance exports not supported yet"
                            );
                        }
                        types::ComponentItem::Type(_) | types::ComponentItem::Resource(_) => {}
                    }
                }
            }
            (_, types::ComponentItem::Type(_) | types::ComponentItem::Resource(_)) => {}
        }
    }
    Ok(())
}

#[instrument(level = "trace", skip_all, ret(level = "trace"))]
pub async fn serve_stateless<S: Serve>(
    handlers: &mut JoinSet<anyhow::Result<()>>,
    srv: S,
    pre: InstancePre<Ctx>,
    engine: &Engine,
    envs: Vec<(Box<str>, Box<str>)>,
) -> anyhow::Result<()> {
    let span = Span::current();
    for (name, types::ComponentExtern { ty, .. }) in
        pre.component().component_type().exports(engine)
    {
        match (name, ty) {
            (name, types::ComponentItem::ComponentFunc(ty)) => {
                let engine = engine.clone();
                let envs = envs.clone();
                info!(?name, "serving root function");
                let invocations = srv
                    .serve_function(
                        move || new_store(&engine, envs.iter().map(|(k, v)| (k, v))),
                        pre.clone(),
                        ty,
                        "",
                        name,
                    )
                    .await?;
                handlers.spawn(
                    async move {
                        handle_invocations(invocations, "root").await;
                        Ok(())
                    }
                    .instrument(span.clone()),
                );
            }
            (_, types::ComponentItem::CoreFunc(_)) => {
                warn!(name, "serving root core function exports not supported yet");
            }
            (_, types::ComponentItem::Module(_)) => {
                warn!(name, "serving root module exports not supported yet");
            }
            (_, types::ComponentItem::Component(_)) => {
                warn!(name, "serving root component exports not supported yet");
            }
            (instance_name, types::ComponentItem::ComponentInstance(ty)) => {
                for (name, types::ComponentExtern { ty, .. }) in ty.exports(engine) {
                    match ty {
                        types::ComponentItem::ComponentFunc(ty) => {
                            let engine = engine.clone();
                            let envs = envs.clone();
                            info!(?name, "serving instance function");
                            let invocations = srv
                                .serve_function(
                                    move || new_store(&engine, envs.iter().map(|(k, v)| (k, v))),
                                    pre.clone(),
                                    ty,
                                    instance_name,
                                    name,
                                )
                                .await?;
                            handlers.spawn(
                                async move {
                                    handle_invocations(invocations, "instance").await;
                                    Ok(())
                                }
                                .instrument(span.clone()),
                            );
                        }
                        types::ComponentItem::CoreFunc(_) => {
                            warn!(
                                instance_name,
                                name, "serving instance core function exports not supported yet"
                            );
                        }
                        types::ComponentItem::Module(_) => {
                            warn!(
                                instance_name,
                                name, "serving instance module exports not supported yet"
                            );
                        }
                        types::ComponentItem::Component(_) => {
                            warn!(
                                instance_name,
                                name, "serving instance component exports not supported yet"
                            );
                        }
                        types::ComponentItem::ComponentInstance(_) => {
                            warn!(
                                instance_name,
                                name, "serving nested instance exports not supported yet"
                            );
                        }
                        types::ComponentItem::Type(_) | types::ComponentItem::Resource(_) => {}
                    }
                }
            }
            (_, types::ComponentItem::Type(_) | types::ComponentItem::Resource(_)) => {}
        }
    }
    Ok(())
}

#[instrument(level = "trace", skip(srv), ret(level = "trace"))]
pub async fn serve<S: Serve>(
    srv: S,
    envs: Vec<(Box<str>, Box<str>)>,
    workload: &str,
) -> anyhow::Result<JoinSet<anyhow::Result<()>>> {
    let (pre, engine, guest_resources) = instantiate_pre(workload).await?;

    let mut handlers = JoinSet::new();
    if guest_resources.is_empty() {
        serve_stateless(&mut handlers, srv, pre, &engine, envs).await?;
    } else {
        serve_shared(
            &mut handlers,
            srv,
            new_store(&engine, envs),
            pre,
            guest_resources,
        )
        .await?;
    }
    Ok(handlers)
}

#[instrument(level = "trace", ret(level = "trace"))]
pub async fn run() -> anyhow::Result<()> {
    wrpc_cli::tracing::init();
    match Command::parse() {
        Command::Tcp(args) => tcp::run(args).await,
    }
}
