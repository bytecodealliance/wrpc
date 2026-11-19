use core::future::poll_fn;
use core::time::Duration;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context as _, anyhow, bail};
use bytes::Bytes;
use clap::Parser;
use futures::StreamExt as _;
use futures::stream::FuturesUnordered;
use tokio::net::TcpListener;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::task::JoinSet;
use tokio::{fs, select, time};
use tracing::{Instrument as _, error, info, instrument, warn};
use url::Url;
use uuid::Uuid;
use wasi_preview1_component_adapter_provider::{
    WASI_SNAPSHOT_PREVIEW1_ADAPTER_NAME, WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER,
};
use wasmtime::component::{
    Accessor, Component, Instance, InstancePre, Linker, ResourceAny, ResourceTable, ResourceType,
    Type, types,
};
use wasmtime::{Config, Engine, Memory, MemoryType, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpCtxView, WasiHttpView};
use wrpc_transport::{Codec, Listener};
use wrpc_wasmtime::{Exports, TupleDecoder, TupleEncoder, call};

mod tcp;

/// Maximum number of connections served concurrently
const MAX_CONNECTIONS: usize = 1 << 10;

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
    /// Guest resources by the handles sent to the peers
    pub resources: HashMap<Uuid, ResourceAny>,
}

fn lift_resource(cx: &mut Ctx, _: &Type, id: Bytes) -> wasmtime::Result<ResourceAny> {
    let id = Uuid::from_slice_le(&id)?;
    cx.resources
        .get(&id)
        .copied()
        .ok_or_else(|| wasmtime::format_err!("resource `{id}` not found"))
}

fn lower_resource(
    cx: &mut Ctx,
    guest: &[ResourceType],
    ty: &Type,
    resource: ResourceAny,
) -> wasmtime::Result<Bytes> {
    let (Type::Own(ty) | Type::Borrow(ty)) = ty else {
        wasmtime::bail!("value type mismatch")
    };
    if !guest.contains(ty) {
        wasmtime::bail!("host resources not supported yet")
    }
    let id = Uuid::now_v7();
    cx.resources.insert(id, resource);
    Ok(Bytes::copy_from_slice(&id.to_bytes_le()))
}

fn collect_resource_exports(
    engine: &Engine,
    ty: types::ComponentItem,
    resources: &mut Vec<ResourceType>,
) {
    match ty {
        types::ComponentItem::Component(ty) => {
            for (.., types::ComponentExtern { ty, .. }) in ty.exports(engine) {
                collect_resource_exports(engine, ty, resources);
            }
        }
        types::ComponentItem::ComponentInstance(ty) => {
            for (.., types::ComponentExtern { ty, .. }) in ty.exports(engine) {
                collect_resource_exports(engine, ty, resources);
            }
        }
        types::ComponentItem::Resource(ty) => resources.push(ty),
        _ => {}
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
    let mut guest_resources = Vec::default();
    collect_resource_exports(
        &engine,
        types::ComponentItem::Component(component.component_type()),
        &mut guest_resources,
    );

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
            resources: HashMap::default(),
        },
    )
}

async fn accept(lis: &TcpListener) -> std::io::Result<Listener<OwnedReadHalf, OwnedWriteHalf>> {
    let (stream, addr) = lis.accept().await?;
    info!(?addr, "accepted connection");
    if let Err(err) = stream.set_nodelay(true) {
        warn!(?err, "failed to set `TCP_NODELAY`");
    }
    let (rx, tx) = stream.into_split();
    Ok(Listener::new(tx, rx))
}

async fn serve_conn(
    mut lis: Listener<OwnedReadHalf, OwnedWriteHalf>,
    timeout: Duration,
    max_len: u32,
    pre: &InstancePre<Ctx>,
    envs: &[(Box<str>, Box<str>)],
    exports: &Exports,
    guest: &[ResourceType],
) -> anyhow::Result<()> {
    while let Ok(next) = time::timeout(timeout, lis.accept()).await {
        let Some((header, tx, rx)) = next.context("failed to read invocation header")? else {
            break;
        };
        let export = exports
            .get(&header.instance, &header.name)
            .with_context(|| format!("`{}#{}` is not exported", header.instance, header.name))?;
        let mut store = new_store(pre.engine(), envs.iter().map(|(k, v)| (k, v)));
        let instance = pre
            .instantiate_async(&mut store)
            .await
            .map_err(anyhow::Error::from)
            .context("failed to instantiate component")?;
        let func = instance
            .get_func(&mut store, export.idx)
            .context("function export not found")?;
        store
            .run_concurrent(async |store| {
                let codec = Codec {
                    enc: TupleEncoder::new(Arc::clone(&export.results)),
                    dec: TupleDecoder::new(Arc::clone(&export.params)).with_max_len(max_len),
                };
                call(
                    store,
                    codec,
                    tx,
                    rx,
                    func,
                    lift_resource,
                    |cx, ty, resource| lower_resource(cx, guest, ty, resource),
                )
                .await?;
                poll_fn(|cx| store.poll_no_interesting_tasks(cx)).await;
                anyhow::Ok(())
            })
            .await
            .map_err(anyhow::Error::from)
            .context("failed to run concurrent event loop")??;
    }
    Ok(())
}

async fn serve_conn_shared(
    store: &Accessor<Ctx>,
    mut lis: Listener<OwnedReadHalf, OwnedWriteHalf>,
    timeout: Duration,
    max_len: u32,
    instance: Instance,
    exports: &Exports,
    guest: &[ResourceType],
) -> anyhow::Result<()> {
    while let Ok(next) = time::timeout(timeout, lis.accept()).await {
        let Some((header, tx, rx)) = next.context("failed to read invocation header")? else {
            break;
        };
        let export = exports
            .get(&header.instance, &header.name)
            .with_context(|| format!("`{}#{}` is not exported", header.instance, header.name))?;
        let func = store
            .with(|store| instance.get_func(store, export.idx))
            .context("function export not found")?;
        let codec = Codec {
            enc: TupleEncoder::new(Arc::clone(&export.results)),
            dec: TupleDecoder::new(Arc::clone(&export.params)).with_max_len(max_len),
        };
        call(
            store,
            codec,
            tx,
            rx,
            func,
            lift_resource,
            |cx, ty, resource| lower_resource(cx, guest, ty, resource),
        )
        .await?;
    }
    Ok(())
}

#[instrument(level = "trace", skip(lis), ret(level = "trace"))]
pub async fn serve(
    lis: TcpListener,
    timeout: Duration,
    max_len: u32,
    envs: Vec<(Box<str>, Box<str>)>,
    workload: &str,
) -> anyhow::Result<()> {
    let (pre, engine, guest) = instantiate_pre(workload).await?;
    let exports = Exports::new(pre.component())?;
    if guest.is_empty() {
        let envs = Arc::<[_]>::from(envs);
        let exports = Arc::new(exports);
        let mut tasks = JoinSet::default();
        loop {
            select! {
                res = accept(&lis), if tasks.len() < MAX_CONNECTIONS => match res {
                    Ok(conn) => {
                        let pre = pre.clone();
                        let envs = Arc::clone(&envs);
                        let exports = Arc::clone(&exports);
                        let guest = Arc::clone(&guest);
                        tasks.spawn(
                            async move {
                                let res =
                                    serve_conn(conn, timeout, max_len, &pre, &envs, &exports, &guest)
                                        .await;
                                if let Err(err) = res {
                                    warn!(?err, "failed to serve connection");
                                }
                            }
                            .in_current_span(),
                        );
                    }
                    Err(err) => error!(?err, "failed to accept connection"),
                },
                Some(res) = tasks.join_next() => {
                    if let Err(err) = res {
                        error!(?err, "failed to join connection task");
                    }
                }
            }
        }
    }
    let mut store = new_store(&engine, envs);
    let instance = pre
        .instantiate_async(&mut store)
        .await
        .map_err(anyhow::Error::from)
        .context("failed to instantiate component")?;
    store
        .run_concurrent(async |store| {
            let mut conns = FuturesUnordered::default();
            loop {
                select! {
                    res = accept(&lis), if conns.len() < MAX_CONNECTIONS => match res {
                        Ok(conn) => conns.push(serve_conn_shared(
                            store, conn, timeout, max_len, instance, &exports, &guest,
                        )),
                        Err(err) => error!(?err, "failed to accept connection"),
                    },
                    Some(res) = conns.next() => {
                        if let Err(err) = res {
                            warn!(?err, "failed to serve connection");
                        }
                    }
                }
            }
        })
        .await
        .map_err(anyhow::Error::from)
        .context("failed to run concurrent event loop")
}

#[instrument(level = "trace", ret(level = "trace"))]
pub async fn run() -> anyhow::Result<()> {
    wrpc_cli::tracing::init();
    match Command::parse() {
        Command::Tcp(args) => tcp::run(args).await,
    }
}
