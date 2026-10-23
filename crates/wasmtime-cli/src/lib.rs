#![allow(clippy::type_complexity)]

use core::future::poll_fn;
use core::ops::Bound;
use core::pin::pin;
use core::time::Duration;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use anyhow::{Context as _, anyhow, bail};
use clap::Parser;
use futures::StreamExt as _;
use tokio::fs;
use tokio::sync::Mutex;
use tokio::task::JoinSet;
use tracing::{Instrument as _, Span, error, info, instrument, warn};
use url::Url;
use wasi_preview1_component_adapter_provider::{
    WASI_SNAPSHOT_PREVIEW1_ADAPTER_NAME, WASI_SNAPSHOT_PREVIEW1_COMMAND_ADAPTER,
    WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER,
};
use wasmtime::component::{Component, InstancePre, Linker, ResourceTable, ResourceType, types};
use wasmtime::{Engine, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpCtxView, WasiHttpView};
use wrpc_transport::{Invoke, Serve};
use wrpc_wasmtime::{
    RemoteResource, ServeExt as _, SharedResourceTable, WrpcCtxView, WrpcView,
    collect_component_resource_exports, collect_component_resource_imports, link_item,
};

mod tcp;

const DEFAULT_TIMEOUT: &str = "10s";

const LINKED_RESOURCES: &[(Bound<&str>, Bound<&str>, &str, fn() -> ResourceType)] = &[
    (
        Bound::Included("wasi:cli/terminal-input@0.2"),
        Bound::Excluded("wasi:cli/terminal-input@0.3"),
        "terminal-input",
        ResourceType::host::<wasmtime_wasi::p2::bindings::cli::terminal_input::TerminalInput>,
    ),
    (
        Bound::Included("wasi:cli/terminal-output@0.2"),
        Bound::Excluded("wasi:cli/terminal-output@0.3"),
        "terminal-output",
        ResourceType::host::<wasmtime_wasi::p2::bindings::cli::terminal_output::TerminalOutput>,
    ),
    (
        Bound::Included("wasi:cli/terminal-input@0.3"),
        Bound::Excluded("wasi:cli/terminal-input@0.4"),
        "terminal-input",
        ResourceType::host::<wasmtime_wasi::p3::bindings::cli::terminal_input::TerminalInput>,
    ),
    (
        Bound::Included("wasi:cli/terminal-output@0.3"),
        Bound::Excluded("wasi:cli/terminal-output@0.4"),
        "terminal-output",
        ResourceType::host::<wasmtime_wasi::p3::bindings::cli::terminal_output::TerminalOutput>,
    ),
    (
        Bound::Included("wasi:filesystem/types@0.2"),
        Bound::Excluded("wasi:filesystem/types@0.3"),
        "descriptor",
        ResourceType::host::<wasmtime_wasi::p2::bindings::filesystem::types::Descriptor>,
    ),
    (
        Bound::Included("wasi:filesystem/types@0.2"),
        Bound::Excluded("wasi:filesystem/types@0.3"),
        "directory-entry-stream",
        ResourceType::host::<wasmtime_wasi::p2::bindings::filesystem::types::DirectoryEntryStream>,
    ),
    (
        Bound::Included("wasi:filesystem/types@0.3"),
        Bound::Excluded("wasi:filesystem/types@0.4"),
        "descriptor",
        ResourceType::host::<wasmtime_wasi::p3::bindings::filesystem::types::Descriptor>,
    ),
    (
        Bound::Included("wasi:http/types@0.2"),
        Bound::Excluded("wasi:http/types@0.3"),
        "fields",
        ResourceType::host::<wasmtime_wasi_http::p2::bindings::http::types::Fields>,
    ),
    (
        Bound::Included("wasi:http/types@0.2"),
        Bound::Excluded("wasi:http/types@0.3"),
        "future-incoming-response",
        ResourceType::host::<wasmtime_wasi_http::p2::bindings::http::types::FutureIncomingResponse>,
    ),
    (
        Bound::Included("wasi:http/types@0.2"),
        Bound::Excluded("wasi:http/types@0.3"),
        "future-trailers",
        ResourceType::host::<wasmtime_wasi_http::p2::bindings::http::types::FutureTrailers>,
    ),
    (
        Bound::Included("wasi:http/types@0.2"),
        Bound::Excluded("wasi:http/types@0.3"),
        "incoming-body",
        ResourceType::host::<wasmtime_wasi_http::p2::bindings::http::types::IncomingBody>,
    ),
    (
        Bound::Included("wasi:http/types@0.2"),
        Bound::Excluded("wasi:http/types@0.3"),
        "incoming-request",
        ResourceType::host::<wasmtime_wasi_http::p2::bindings::http::types::IncomingRequest>,
    ),
    (
        Bound::Included("wasi:http/types@0.2"),
        Bound::Excluded("wasi:http/types@0.3"),
        "incoming-response",
        ResourceType::host::<wasmtime_wasi_http::p2::bindings::http::types::IncomingResponse>,
    ),
    (
        Bound::Included("wasi:http/types@0.2"),
        Bound::Excluded("wasi:http/types@0.3"),
        "outgoing-body",
        ResourceType::host::<wasmtime_wasi_http::p2::bindings::http::types::OutgoingBody>,
    ),
    (
        Bound::Included("wasi:http/types@0.2"),
        Bound::Excluded("wasi:http/types@0.3"),
        "outgoing-request",
        ResourceType::host::<wasmtime_wasi_http::p2::bindings::http::types::OutgoingRequest>,
    ),
    (
        Bound::Included("wasi:http/types@0.2"),
        Bound::Excluded("wasi:http/types@0.3"),
        "outgoing-response",
        ResourceType::host::<wasmtime_wasi_http::p2::bindings::http::types::OutgoingResponse>,
    ),
    (
        Bound::Included("wasi:http/types@0.2"),
        Bound::Excluded("wasi:http/types@0.3"),
        "request-options",
        ResourceType::host::<wasmtime_wasi_http::p2::bindings::http::types::RequestOptions>,
    ),
    (
        Bound::Included("wasi:http/types@0.2"),
        Bound::Excluded("wasi:http/types@0.3"),
        "response-outparam",
        ResourceType::host::<wasmtime_wasi_http::p2::bindings::http::types::ResponseOutparam>,
    ),
    (
        Bound::Included("wasi:http/types@0.3"),
        Bound::Excluded("wasi:http/types@0.4"),
        "fields",
        ResourceType::host::<wasmtime_wasi_http::p3::bindings::http::types::Fields>,
    ),
    (
        Bound::Included("wasi:http/types@0.3"),
        Bound::Excluded("wasi:http/types@0.4"),
        "request",
        ResourceType::host::<wasmtime_wasi_http::p3::bindings::http::types::Request>,
    ),
    (
        Bound::Included("wasi:http/types@0.3"),
        Bound::Excluded("wasi:http/types@0.4"),
        "request-options",
        ResourceType::host::<wasmtime_wasi_http::p3::bindings::http::types::RequestOptions>,
    ),
    (
        Bound::Included("wasi:http/types@0.3"),
        Bound::Excluded("wasi:http/types@0.4"),
        "response",
        ResourceType::host::<wasmtime_wasi_http::p3::bindings::http::types::Response>,
    ),
    (
        Bound::Included("wasi:io/error@0.2"),
        Bound::Excluded("wasi:io/error@0.3"),
        "error",
        ResourceType::host::<wasmtime_wasi::p2::bindings::io::error::Error>,
    ),
    (
        Bound::Included("wasi:io/poll@0.2"),
        Bound::Excluded("wasi:io/poll@0.3"),
        "pollable",
        ResourceType::host::<wasmtime_wasi::p2::bindings::io::poll::Pollable>,
    ),
    (
        Bound::Included("wasi:io/streams@0.2"),
        Bound::Excluded("wasi:io/streams@0.3"),
        "input-stream",
        ResourceType::host::<wasmtime_wasi::p2::bindings::io::streams::InputStream>,
    ),
    (
        Bound::Included("wasi:io/streams@0.2"),
        Bound::Excluded("wasi:io/streams@0.3"),
        "output-stream",
        ResourceType::host::<wasmtime_wasi::p2::bindings::io::streams::OutputStream>,
    ),
    (
        Bound::Included("wasi:sockets/ip-name-lookup@0.2"),
        Bound::Excluded("wasi:sockets/ip-name-lookup@0.3"),
        "resolve-address-stream",
        ResourceType::host::<
            wasmtime_wasi::p2::bindings::sockets::ip_name_lookup::ResolveAddressStream,
        >,
    ),
    (
        Bound::Included("wasi:sockets/network@0.2"),
        Bound::Excluded("wasi:sockets/network@0.3"),
        "network",
        ResourceType::host::<wasmtime_wasi::p2::bindings::sockets::network::Network>,
    ),
    (
        Bound::Included("wasi:sockets/tcp@0.2"),
        Bound::Excluded("wasi:sockets/tcp@0.3"),
        "tcp-socket",
        ResourceType::host::<wasmtime_wasi::p2::bindings::sockets::tcp::TcpSocket>,
    ),
    (
        Bound::Included("wasi:sockets/udp@0.2"),
        Bound::Excluded("wasi:sockets/udp@0.3"),
        "incoming-datagram-stream",
        ResourceType::host::<wasmtime_wasi::p2::bindings::sockets::udp::IncomingDatagramStream>,
    ),
    (
        Bound::Included("wasi:sockets/udp@0.2"),
        Bound::Excluded("wasi:sockets/udp@0.3"),
        "outgoing-datagram-stream",
        ResourceType::host::<wasmtime_wasi::p2::bindings::sockets::udp::OutgoingDatagramStream>,
    ),
    (
        Bound::Included("wasi:sockets/udp@0.2"),
        Bound::Excluded("wasi:sockets/udp@0.3"),
        "udp-socket",
        ResourceType::host::<wasmtime_wasi::p2::bindings::sockets::udp::UdpSocket>,
    ),
    (
        Bound::Included("wasi:sockets/types@0.3"),
        Bound::Excluded("wasi:sockets/types@0.4"),
        "tcp-socket",
        ResourceType::host::<wasmtime_wasi::p3::bindings::sockets::types::TcpSocket>,
    ),
    (
        Bound::Included("wasi:sockets/types@0.3"),
        Bound::Excluded("wasi:sockets/types@0.4"),
        "udp-socket",
        ResourceType::host::<wasmtime_wasi::p3::bindings::sockets::types::UdpSocket>,
    ),
    (
        Bound::Included("wrpc:rpc/context@0.1.0"),
        Bound::Included("wrpc:rpc/context@0.1.0"),
        "context",
        ResourceType::host::<wrpc_wasmtime::bindings::rpc::context::Context>,
    ),
    (
        Bound::Included("wrpc:rpc/error@0.1.0"),
        Bound::Included("wrpc:rpc/error@0.1.0"),
        "error",
        ResourceType::host::<wrpc_wasmtime::bindings::rpc::error::Error>,
    ),
    (
        Bound::Included("wrpc:rpc/transport@0.1.0"),
        Bound::Included("wrpc:rpc/transport@0.1.0"),
        "incoming-channel",
        ResourceType::host::<wrpc_wasmtime::bindings::rpc::transport::IncomingChannel>,
    ),
    (
        Bound::Included("wrpc:rpc/transport@0.1.0"),
        Bound::Included("wrpc:rpc/transport@0.1.0"),
        "invocation",
        ResourceType::host::<wrpc_wasmtime::bindings::rpc::transport::Invocation>,
    ),
    (
        Bound::Included("wrpc:rpc/transport@0.1.0"),
        Bound::Included("wrpc:rpc/transport@0.1.0"),
        "outgoing-channel",
        ResourceType::host::<wrpc_wasmtime::bindings::rpc::transport::OutgoingChannel>,
    ),
];

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

pub struct WrpcCtx<C: Invoke> {
    pub wrpc: C,
    pub cx: C::Context,
    pub shared_resources: SharedResourceTable,
    pub timeout: Duration,
}

pub struct Ctx<C: Invoke> {
    pub table: ResourceTable,
    pub wasi: WasiCtx,
    pub http: WasiHttpCtx,
    pub wrpc: WrpcCtx<C>,
}

impl<C> wrpc_wasmtime::WrpcCtx<C> for WrpcCtx<C>
where
    C: Invoke,
    C::Context: Clone,
{
    fn context(&self) -> C::Context {
        self.cx.clone()
    }

    fn client(&self) -> &C {
        &self.wrpc
    }

    fn shared_resources(&mut self) -> &mut SharedResourceTable {
        &mut self.shared_resources
    }

    fn timeout(&self) -> Option<Duration> {
        Some(self.timeout)
    }
}

impl<C> WrpcView for Ctx<C>
where
    C: Invoke,
    C::Context: Clone,
{
    type Invoke = C;

    fn wrpc(&mut self) -> WrpcCtxView<'_, Self::Invoke> {
        WrpcCtxView {
            ctx: &mut self.wrpc,
            table: &mut self.table,
        }
    }
}

impl<C: Invoke> WasiView for Ctx<C> {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl<C: Invoke> WasiHttpView for Ctx<C> {
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
    let mut config = wasmtime::Config::new();
    config.wasm_memory64(true);
    config.memory_reservation(1 << BITS_TO_TEST);
    let engine = wasmtime::Engine::new(&config)?;
    let mut store = wasmtime::Store::new(&engine, ());
    // NB: the maximum size is in wasm pages to take out the 16-bits of wasm
    // page size here from the maximum size.
    let ty = wasmtime::MemoryType::new64(0, Some(1 << (BITS_TO_TEST - 16)));
    if wasmtime::Memory::new(&mut store, ty).is_ok() {
        Ok(Some(true))
    } else {
        Ok(None)
    }
}

fn is_0_x(version: &str, x: u64, min_patch: u64) -> bool {
    match version.parse() {
        Ok(semver::Version {
            major,
            minor,
            patch,
            pre,
            build,
        }) => major == 0 && minor == x && patch >= min_patch && pre.is_empty() && build.is_empty(),
        _ => false,
    }
}

fn is_0_2(version: &str, min_patch: u64) -> bool {
    is_0_x(version, 2, min_patch)
}

fn is_0_3(version: &str) -> bool {
    is_0_x(version, 3, 0)
}

fn is_linked(name: &str) -> bool {
    match name.split_once('/').map(|(pkg, suffix)| {
        suffix
            .split_once('@')
            .map_or((pkg, suffix, None), |(iface, version)| {
                (pkg, iface, Some(version))
            })
    }) {
        Some(("wrpc:rpc", "transport" | "error" | "context" | "invoker", Some("0.1.0"))) => {}
        Some((
            "wasi:cli",
            "environment" | "exit" | "stderr" | "stdin" | "stdout" | "terminal-input"
            | "terminal-output" | "terminal-stderr" | "terminal-stdin" | "terminal-stdout",
            Some(version),
        )) if is_0_2(version, 0) => {}
        Some((
            "wasi:cli",
            "environment" | "exit" | "stderr" | "stdin" | "stdout" | "terminal-input"
            | "terminal-output" | "terminal-stderr" | "terminal-stdin" | "terminal-stdout"
            | "types",
            Some(version),
        )) if is_0_3(version) => {}
        Some(("wasi:clocks", "monotonic-clock" | "wall-clock", Some(version)))
            if is_0_2(version, 0) => {}
        Some(("wasi:clocks", "timezone", Some(version))) if is_0_2(version, 1) => {}
        Some(("wasi:clocks", "monotonic-clock" | "system-clock" | "types", Some(version)))
            if is_0_3(version) => {}
        Some(("wasi:filesystem", "preopens" | "types", Some(version)))
            if is_0_2(version, 0) || is_0_3(version) => {}
        Some(("wasi:http", "incoming-handler" | "outgoing-handler" | "types", Some(version)))
            if is_0_2(version, 0) => {}
        Some(("wasi:http", "client" | "types", Some(version))) if is_0_3(version) => {}
        Some(("wasi:io", "error" | "poll" | "streams", Some(version))) if is_0_2(version, 0) => {}
        Some(("wasi:random", "insecure-seed" | "insecure" | "random", Some(version)))
            if is_0_2(version, 0) || is_0_3(version) => {}
        Some((
            "wasi:sockets",
            "instance-network" | "ip-name-lookup" | "network" | "tcp-create-socket" | "tcp"
            | "udp-create-socket" | "udp",
            Some(version),
        )) if is_0_2(version, 0) => {}
        Some(("wasi:sockets", "ip-name-lookup" | "types", Some(version))) if is_0_3(version) => {}
        _ => return false,
    }
    true
}

#[instrument(level = "trace", skip(adapter))]
async fn instantiate_pre<C>(
    adapter: &[u8],
    workload: &str,
) -> anyhow::Result<(
    InstancePre<Ctx<C>>,
    Engine,
    Arc<[ResourceType]>,
    Arc<HashMap<Box<str>, HashMap<Box<str>, (ResourceType, ResourceType)>>>,
)>
where
    C: Invoke + Clone + 'static,
    C::Context: Clone + 'static,
{
    let mut opts = wasmtime_cli_flags::CommonOptions::new();
    let mut config = opts
        .config(use_pooling_allocator_by_default().unwrap_or(None))
        .map_err(anyhow::Error::from)
        .context("failed to construct Wasmtime config")?;
    config.wasm_component_model(true);
    let engine = wasmtime::Engine::new(&config)
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
            .adapter(WASI_SNAPSHOT_PREVIEW1_ADAPTER_NAME, adapter)
            .context("failed to add WASI adapter")?
            .encode()
            .context("failed to encode a component")?
    } else {
        wasm
    };

    let component = Component::new(&engine, wasm)
        .map_err(anyhow::Error::from)
        .context("failed to compile component")?;

    let mut linker = Linker::<Ctx<C>>::new(&engine);
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
    wrpc_wasmtime::rpc::add_to_linker(&mut linker)
        .map_err(anyhow::Error::from)
        .context("failed to link `wrpc:rpc`")?;

    let ty = component.component_type();
    let mut host_resources = BTreeMap::default();
    let mut guest_resources = Vec::new();
    collect_component_resource_imports(&engine, &ty, &mut host_resources);
    collect_component_resource_exports(&engine, &ty, &mut guest_resources);
    let linked_resources = LINKED_RESOURCES
        .iter()
        .flat_map(|(start, end, name, host_ty)| {
            host_resources
                .range::<str, _>((*start, *end))
                .filter(|(instance, _)| is_linked(instance))
                .filter_map(move |(_, instance)| instance.get(*name))
                .map(move |ty| (*ty, host_ty()))
        })
        .collect::<Box<[_]>>();
    let host_resources = host_resources
        .into_iter()
        .map(|(instance, resources)| {
            let resources = resources
                .into_iter()
                .map(|(name, ty)| {
                    let host_ty = linked_resources
                        .iter()
                        .find_map(|(linked_ty, host_ty)| (*linked_ty == ty).then_some(*host_ty))
                        .unwrap_or_else(ResourceType::host::<RemoteResource>);
                    (name, (ty, host_ty))
                })
                .collect::<HashMap<_, _>>();
            (instance, resources)
        })
        .collect::<HashMap<_, _>>();
    let host_resources = Arc::from(host_resources);
    let guest_resources = Arc::from(guest_resources);
    for (name, types::ComponentExtern { ty: item, .. }) in ty.imports(&engine) {
        if is_linked(name) {
            continue;
        }
        if let Err(err) = link_item(
            &engine,
            &mut linker.root(),
            Arc::clone(&guest_resources),
            Arc::clone(&host_resources),
            item,
            "",
            name,
        ) {
            error!(?err, "failed to polyfill instance");
        }
    }

    let pre = linker
        .instantiate_pre(&component)
        .map_err(anyhow::Error::from)
        .context("failed to pre-instantiate component")?;
    Ok((pre, engine, guest_resources, host_resources))
}

fn new_store<C: Invoke>(
    engine: &Engine,
    wrpc: C,
    cx: C::Context,
    envs: impl IntoIterator<Item = (impl AsRef<str>, impl AsRef<str>)>,
    arg0: &str,
    timeout: Duration,
) -> wasmtime::Store<Ctx<C>> {
    let mut wasi = WasiCtxBuilder::new();
    wasi.inherit_env()
        .inherit_stdio()
        .inherit_network()
        .allow_ip_name_lookup(true)
        .allow_tcp(true)
        .allow_udp(true)
        .args(&[arg0]);
    for (k, v) in envs {
        wasi.env(k, v);
    }
    Store::new(
        engine,
        Ctx {
            wasi: wasi.build(),
            http: WasiHttpCtx::new(),
            table: ResourceTable::new(),
            wrpc: WrpcCtx {
                wrpc,
                cx,
                shared_resources: SharedResourceTable::default(),
                timeout,
            },
        },
    )
}

#[instrument(level = "trace", skip(clt, cx), ret(level = "trace"))]
pub async fn handle_run<C>(
    clt: C,
    cx: C::Context,
    timeout: Duration,
    envs: Vec<(Box<str>, Box<str>)>,
    workload: &str,
) -> anyhow::Result<()>
where
    C: Invoke + Clone + 'static,
    C::Context: Clone + 'static,
{
    let (pre, engine, _, _) =
        instantiate_pre(WASI_SNAPSHOT_PREVIEW1_COMMAND_ADAPTER, workload).await?;
    let mut store = new_store(&engine, clt, cx, envs, "command.wasm", timeout);
    if pre
        .component()
        .get_export_index(None, "wasi:cli/run@0.3.0")
        .is_some()
    {
        let cmd = wasmtime_wasi::p3::bindings::CommandPre::new(pre)
            .map_err(anyhow::Error::from)
            .context("failed to construct `command` instance")?;
        let cmd = cmd
            .instantiate_async(&mut store)
            .await
            .map_err(anyhow::Error::from)
            .context("failed to instantiate `command`")?;
        let res = store
            .run_concurrent(async move |store| -> wasmtime::Result<_> {
                let res = cmd.wasi_cli_run().call_run(store).await?;
                poll_fn(|cx| store.poll_no_interesting_tasks(cx)).await;
                Ok(res)
            })
            .await
            .map_err(anyhow::Error::from)
            .context("failed to run concurrent event loop")?;
        let res = res
            .map_err(anyhow::Error::from)
            .context("failed to run component")?;
        return res.map_err(|()| anyhow!("component failed"));
    }
    let cmd = wasmtime_wasi::p2::bindings::CommandPre::new(pre)
        .map_err(anyhow::Error::from)
        .context("failed to construct `command` instance")?;
    let cmd = cmd
        .instantiate_async(&mut store)
        .await
        .map_err(anyhow::Error::from)
        .context("failed to instantiate `command`")?;
    let res = cmd
        .wasi_cli_run()
        .call_run(&mut store)
        .await
        .map_err(anyhow::Error::from)
        .context("failed to run component")?;
    res.map_err(|()| anyhow!("component failed"))
}

#[instrument(level = "trace", skip_all, ret(level = "trace"))]
pub async fn serve_shared<C, S>(
    handlers: &mut JoinSet<()>,
    srv: S,
    mut store: wasmtime::Store<Ctx<C>>,
    pre: InstancePre<Ctx<C>>,
    guest_resources: Arc<[ResourceType]>,
    host_resources: Arc<HashMap<Box<str>, HashMap<Box<str>, (ResourceType, ResourceType)>>>,
) -> anyhow::Result<()>
where
    C: Invoke + 'static,
    C::Context: Clone,
    S: Serve,
{
    let span = Span::current();
    let instance = pre
        .instantiate_async(&mut store)
        .await
        .map_err(anyhow::Error::from)
        .context("failed to instantiate component")?;
    let engine = store.engine().clone();
    let io_stream_resources: Arc<[ResourceType]> =
        wrpc_wasmtime::paths::wasi_io_stream_resources(&engine, &pre.component().component_type())
            .into();
    let store = Arc::new(Mutex::new(store));
    for (name, types::ComponentExtern { ty, .. }) in
        pre.component().component_type().exports(&engine)
    {
        match (name, ty) {
            (name, types::ComponentItem::ComponentFunc(ty)) => {
                info!(?name, "serving root function");
                let invocations = srv
                    .serve_function_shared(
                        Arc::clone(&store),
                        instance,
                        Arc::clone(&guest_resources),
                        Arc::clone(&host_resources),
                        Arc::clone(&io_stream_resources),
                        ty,
                        "",
                        name,
                    )
                    .await?;
                handlers.spawn(
                    async move {
                        let mut invocations = pin!(invocations);
                        while let Some(invocation) = invocations.next().await {
                            match invocation {
                                Ok((_, fut)) => {
                                    info!("serving root function invocation");
                                    match fut.await {
                                        Err(err) => {
                                            warn!(?err, "failed to serve root function invocation");
                                        }
                                        _ => {
                                            info!("successfully served root function invocation");
                                        }
                                    }
                                }
                                Err(err) => {
                                    error!(?err, "failed to accept root function invocation");
                                }
                            }
                        }
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
                for (name, types::ComponentExtern { ty, .. }) in ty.exports(&engine) {
                    match ty {
                        types::ComponentItem::ComponentFunc(ty) => {
                            info!(?name, "serving instance function");
                            let invocations = srv
                                .serve_function_shared(
                                    Arc::clone(&store),
                                    instance,
                                    Arc::clone(&guest_resources),
                                    Arc::clone(&host_resources),
                                    Arc::clone(&io_stream_resources),
                                    ty,
                                    instance_name,
                                    name,
                                )
                                .await?;
                            handlers.spawn(async move {
                                let mut invocations = pin!(invocations);
                                while let Some(invocation) = invocations.next().await {
                                    match invocation {
                                        Ok((_, fut)) => {
                                            info!("serving instance function invocation");
                                            match fut.await { Err(err) => {
                                                warn!(
                                                    ?err,
                                                    "failed to serve instance function invocation"
                                                );
                                            } _ => {
                                                info!(
                                                    "successfully served instance function invocation"
                                                );
                                            }}
                                        }
                                        Err(err) => {
                                            error!(
                                                ?err,
                                                "failed to accept instance function invocation"
                                            );
                                        }
                                    }
                                }
                            }
                            .instrument(span.clone()));
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
#[allow(clippy::too_many_arguments)]
pub async fn serve_stateless<C, S>(
    handlers: &mut JoinSet<()>,
    srv: S,
    clt: C,
    cx: C::Context,
    pre: InstancePre<Ctx<C>>,
    host_resources: Arc<HashMap<Box<str>, HashMap<Box<str>, (ResourceType, ResourceType)>>>,
    engine: &Engine,
    timeout: Duration,
    envs: Vec<(Box<str>, Box<str>)>,
) -> anyhow::Result<()>
where
    C: Invoke + Clone + 'static,
    C::Context: Clone + 'static,
    S: Serve,
{
    let span = Span::current();
    for (name, types::ComponentExtern { ty, .. }) in
        pre.component().component_type().exports(engine)
    {
        match (name, ty) {
            (name, types::ComponentItem::ComponentFunc(ty)) => {
                let clt = clt.clone();
                let cx = cx.clone();
                let engine = engine.clone();
                let envs = envs.clone();
                info!(?name, "serving root function");
                let invocations = srv
                    .serve_function(
                        move || {
                            new_store(
                                &engine,
                                clt.clone(),
                                cx.clone(),
                                envs.iter().map(|(k, v)| (k, v)),
                                "reactor.wasm",
                                timeout,
                            )
                        },
                        pre.clone(),
                        Arc::clone(&host_resources),
                        ty,
                        "",
                        name,
                    )
                    .await?;
                handlers.spawn(
                    async move {
                        let mut invocations = pin!(invocations);
                        while let Some(invocation) = invocations.next().await {
                            match invocation {
                                Ok((_, fut)) => {
                                    info!("serving root function invocation");
                                    match fut.await {
                                        Err(err) => {
                                            warn!(?err, "failed to serve root function invocation");
                                        }
                                        _ => {
                                            info!("successfully served root function invocation");
                                        }
                                    }
                                }
                                Err(err) => {
                                    error!(?err, "failed to accept root function invocation");
                                }
                            }
                        }
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
                            let clt = clt.clone();
                            let engine = engine.clone();
                            let cx = cx.clone();
                            let envs = envs.clone();
                            info!(?name, "serving instance function");
                            let invocations = srv
                                .serve_function(
                                    move || {
                                        new_store(
                                            &engine,
                                            clt.clone(),
                                            cx.clone(),
                                            envs.iter().map(|(k, v)| (k, v)),
                                            "reactor.wasm",
                                            timeout,
                                        )
                                    },
                                    pre.clone(),
                                    Arc::clone(&host_resources),
                                    ty,
                                    instance_name,
                                    name,
                                )
                                .await?;
                            handlers.spawn(async move {
                                let mut invocations = pin!(invocations);
                                while let Some(invocation) = invocations.next().await {
                                    match invocation {
                                        Ok((_, fut)) => {
                                            info!("serving instance function invocation");
                                            match fut.await { Err(err) => {
                                                warn!(
                                                    ?err,
                                                    "failed to serve instance function invocation"
                                                );
                                            } _ => {
                                                info!(
                                                    "successfully served instance function invocation"
                                                );
                                            }}
                                        }
                                        Err(err) => {
                                            error!(
                                                ?err,
                                                "failed to accept instance function invocation"
                                            );
                                        }
                                    }
                                }
                            }.instrument(span.clone()));
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

#[instrument(level = "trace", skip(srv, clt, cx), ret(level = "trace"))]
pub async fn serve<C, S>(
    srv: S,
    clt: C,
    cx: C::Context,
    timeout: Duration,
    envs: Vec<(Box<str>, Box<str>)>,
    workload: &str,
) -> anyhow::Result<JoinSet<()>>
where
    C: Invoke + Clone + 'static,
    C::Context: Clone + 'static,
    S: Serve,
{
    let (pre, engine, guest_resources, host_resources) =
        instantiate_pre(WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER, workload).await?;

    let mut handlers = JoinSet::new();
    if guest_resources.is_empty() {
        serve_stateless(
            &mut handlers,
            srv,
            clt,
            cx,
            pre,
            host_resources,
            &engine,
            timeout,
            envs,
        )
        .await?;
    } else {
        serve_shared(
            &mut handlers,
            srv,
            new_store(&engine, clt, cx, envs, "reactor.wasm", timeout),
            pre,
            guest_resources,
            host_resources,
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
