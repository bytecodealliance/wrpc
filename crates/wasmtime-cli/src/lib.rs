#![allow(clippy::type_complexity)]

use core::pin::{Pin, pin};
use core::time::Duration;

use std::sync::Arc;

use anyhow::{Context as _, anyhow, bail};
use clap::Parser;
use futures::{Stream, StreamExt as _};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::task::JoinSet;
use tokio::{fs, select, spawn, time, try_join};
use tracing::{Instrument as _, error, info, instrument, warn};
use url::Url;
use wasi_preview1_component_adapter_provider::{
    WASI_SNAPSHOT_PREVIEW1_ADAPTER_NAME, WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER,
};
use wasmtime::component::{Component, InstancePre, Linker, ResourceTable, ResourceType};
use wasmtime::{Config, Engine, Memory, MemoryType, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpCtxView, WasiHttpView};
use wrpc_transport::{Header, accept};
use wrpc_wasmtime::{
    InstancePreExports, ResourceTypes, SharedExports, SharedResourceTable, SharedStore,
    WrpcCtxView, WrpcView, collect_component_resource_exports,
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

#[instrument(level = "trace", skip_all, ret(level = "trace"))]
async fn serve_exports<C, I, O, E>(
    incoming: impl Stream<Item = Result<(C, O, I), E>>,
    timeout: Duration,
    event_loop: impl Future<Output = anyhow::Result<()>>,
    handle: impl Fn(
        C,
        &str,
        &str,
        O,
        I,
    ) -> Result<Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>, (C, O, I)>
    + Send
    + Sync
    + 'static,
) -> anyhow::Result<()>
where
    C: Send + 'static,
    I: AsyncRead + Send + Unpin + 'static,
    O: AsyncWrite + Send + Unpin + 'static,
    E: core::error::Error + Send + Sync + 'static,
{
    let serve = async move {
        let handle = Arc::new(handle);
        let mut incoming = pin!(incoming);
        let mut tasks = JoinSet::default();
        let mut done = false;
        loop {
            select! {
                res = incoming.next(), if !done => match res {
                    Some(Ok((cx, tx, mut rx))) => {
                        let handle = Arc::clone(&handle);
                        tasks.spawn(
                            async move {
                                let Header { instance, name } = match time::timeout(
                                    timeout,
                                    accept(&mut rx),
                                )
                                .await
                                {
                                    Ok(Ok(header)) => header,
                                    Ok(Err(err)) => {
                                        error!(?err, "failed to read invocation header");
                                        return;
                                    }
                                    Err(..) => {
                                        error!("timed out reading invocation header");
                                        return;
                                    }
                                };
                                match handle(cx, &instance, &name, tx, rx) {
                                    Ok(fut) => {
                                        info!("serving function invocation");
                                        if let Err(err) = fut.await {
                                            warn!(?err, "failed to serve function invocation");
                                        } else {
                                            info!("successfully served function invocation");
                                        }
                                    }
                                    Err(..) => error!(
                                        instance,
                                        func = name,
                                        "function is not exported"
                                    ),
                                }
                            }
                            .in_current_span(),
                        );
                    }
                    Some(Err(err)) => error!(?err, "failed to accept invocation"),
                    None => done = true,
                },
                Some(res) = tasks.join_next() => {
                    if let Err(err) = res {
                        error!(?err, "failed to join invocation task");
                    }
                }
                else => return anyhow::Ok(()),
            }
        }
    };
    try_join!(serve, event_loop)?;
    Ok(())
}

#[instrument(level = "trace", skip(incoming), ret(level = "trace"))]
pub async fn serve<C, I, O, E>(
    incoming: impl Stream<Item = Result<(C, O, I), E>>,
    timeout: Duration,
    envs: Vec<(Box<str>, Box<str>)>,
    workload: &str,
) -> anyhow::Result<()>
where
    C: Send + 'static,
    I: AsyncRead + Send + Unpin + 'static,
    O: AsyncWrite + Send + Unpin + 'static,
    E: core::error::Error + Send + Sync + 'static,
{
    let (pre, engine, guest_resources) = instantiate_pre(workload).await?;
    if guest_resources.is_empty() {
        let exports = InstancePreExports::new(
            move || new_store(&engine, envs.iter().map(|(k, v)| (k, v))),
            pre,
        )?;
        return serve_exports(
            incoming,
            timeout,
            async { Ok(()) },
            move |cx, instance, func, tx, rx| exports.handle(cx, instance, func, tx, rx),
        )
        .await;
    }
    let mut store = new_store(&engine, envs);
    let instance = pre
        .instantiate_async(&mut store)
        .await
        .map_err(anyhow::Error::from)
        .context("failed to instantiate component")?;
    let resources = ResourceTypes {
        guest: guest_resources,
        io_streams: wrpc_wasmtime::paths::wasi_io_stream_resources(
            &engine,
            &pre.component().component_type(),
        )
        .into(),
    };
    let (store, event_loop) = SharedStore::new(store);
    let exports = SharedExports::new(store, instance, pre.component(), resources)?;
    let event_loop = spawn(event_loop.in_current_span());
    serve_exports(
        incoming,
        timeout,
        async {
            match event_loop.await {
                Ok(res) => res,
                Err(err) => Err(err).context("failed to join event loop"),
            }
        },
        move |cx, instance, func, tx, rx| exports.handle(cx, instance, func, tx, rx),
    )
    .await
}

#[instrument(level = "trace", ret(level = "trace"))]
pub async fn run() -> anyhow::Result<()> {
    wrpc_cli::tracing::init();
    match Command::parse() {
        Command::Tcp(args) => tcp::run(args).await,
    }
}

#[cfg(test)]
mod tests {
    use futures::stream;
    use tokio::io::{empty, sink};

    use super::*;

    #[tokio::test]
    async fn serve_exports_ends() -> anyhow::Result<()> {
        serve_exports(
            stream::iter([Ok::<_, std::io::Error>(((), sink(), empty()))]),
            Duration::from_secs(1),
            async { Ok(()) },
            |cx, _, _, tx, rx| Err((cx, tx, rx)),
        )
        .await
    }
}
