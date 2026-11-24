use core::pin::Pin;
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
use tokio::{fs, select};
use tokio_io_timeout::{TimeoutReader, TimeoutWriter};
use tracing::{error, info, instrument, warn};
use tracing_subscriber::EnvFilter;
use url::Url;
use uuid::Uuid;
use wasi_preview1_component_adapter_provider::{
    WASI_SNAPSHOT_PREVIEW1_ADAPTER_NAME, WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER,
};
use wasmtime::component::{
    self, Accessor, Component, Func, Instance, InstancePre, Linker, ResourceAny, ResourceTable,
    ResourceType, Type, types,
};
use wasmtime::{Config, Engine, Memory, MemoryType, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpCtxView, WasiHttpView};
use wrpc_core::{Codec, HeaderDecodeError, Listener};
use wrpc_wasmtime::{Export, Exports, TupleDecoder, TupleEncoder, Val, lift, lower};

mod tcp;

/// Maximum number of connections served concurrently
const MAX_CONNECTIONS: usize = 1 << 10;

type Conn =
    Listener<Pin<Box<TimeoutReader<OwnedReadHalf>>>, Pin<Box<TimeoutWriter<OwnedWriteHalf>>>>;

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

fn lift_resource(
    cx: &Ctx,
    used: &mut HashMap<Uuid, bool>,
    ty: &Type,
    id: Bytes,
) -> wasmtime::Result<ResourceAny> {
    let (Type::Own(expected) | Type::Borrow(expected)) = ty else {
        wasmtime::bail!("value type mismatch")
    };
    let id = Uuid::from_slice_le(&id)?;
    let Some(resource) = cx.resources.get(&id).copied() else {
        wasmtime::bail!("resource `{id}` not found")
    };
    if resource.ty() != *expected {
        wasmtime::bail!("resource `{id}` type mismatch")
    }
    let own = matches!(ty, Type::Own(..));
    if let Some(prev) = used.insert(id, own)
        && (own || prev)
    {
        wasmtime::bail!("owned resource `{id}` used more than once")
    }
    Ok(resource)
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
) -> anyhow::Result<(InstancePre<Ctx>, Engine, Vec<ResourceType>)> {
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
    Ok((pre, engine, guest_resources))
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

async fn accept(lis: &TcpListener, timeout: Duration) -> std::io::Result<Conn> {
    let (stream, addr) = lis.accept().await?;
    info!(?addr, "accepted connection");
    if let Err(err) = stream.set_nodelay(true) {
        warn!(?err, "failed to set `TCP_NODELAY`");
    }
    let (rx, tx) = stream.into_split();
    let mut rx = TimeoutReader::new(rx);
    rx.set_timeout(Some(timeout));
    let mut tx = TimeoutWriter::new(tx);
    tx.set_timeout(Some(timeout));
    Ok(Listener::new(Box::pin(tx), Box::pin(rx)))
}

async fn call(
    store: &Accessor<Ctx>,
    func: Func,
    export: &Export,
    guest: &[ResourceType],
    params: Vec<Val>,
) -> anyhow::Result<Vec<Val>> {
    let params = store.with(|mut store| -> wasmtime::Result<_> {
        let ty = func.ty(&store);
        let mut used = HashMap::default();
        let params = lift(
            &mut store,
            ty.params().map(|(.., ty)| ty),
            params,
            |cx, ty, id| lift_resource(cx, &mut used, ty, id),
        )?;
        let cx = store.data_mut();
        for (id, own) in used {
            if own {
                cx.resources.remove(&id);
            }
        }
        Ok(params)
    })?;
    let mut results = vec![component::Val::Bool(false); export.results.len()];
    func.call_concurrent(store, &params, &mut results)
        .await
        .map_err(anyhow::Error::from)
        .context("failed to call function")?;
    store
        .with(|store| {
            lower(store, export.results.iter(), results, |cx, ty, resource| {
                lower_resource(cx, guest, ty, resource)
            })
        })
        .map_err(anyhow::Error::from)
}

async fn serve_conn(
    store: &Accessor<Ctx>,
    mut conn: Conn,
    max_len: u32,
    instance: Instance,
    exports: &Exports,
    guest: &[ResourceType],
) -> anyhow::Result<()> {
    loop {
        let (header, srv) = match conn.accept().await {
            Ok(Some(next)) => next,
            Ok(None) => break,
            Err(HeaderDecodeError::Io(err)) if err.kind() == std::io::ErrorKind::TimedOut => break,
            Err(err) => return Err(err).context("failed to read invocation header"),
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
        codec
            .serve(&mut srv.tx, &mut srv.rx, async |params| {
                call(store, func, export, guest, params)
                    .await
                    .map_err(std::io::Error::other)
            })
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
                    res = accept(&lis, timeout), if conns.len() < MAX_CONNECTIONS => match res {
                        Ok(conn) => conns.push(serve_conn(
                            store, conn, max_len, instance, &exports, &guest,
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
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .compact()
        .without_time()
        .with_env_filter(filter)
        .init();
    match Command::parse() {
        Command::Tcp(args) => tcp::run(args).await,
    }
}
