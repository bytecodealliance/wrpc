use core::pin::pin;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _};
use clap::Parser;
use futures::StreamExt as _;
use tokio::fs;
use tokio::task::JoinSet;
use tracing::{error, info, instrument, trace, warn, Instrument as _};
use url::Url;
use wasmcloud_component_adapters::{
    WASI_PREVIEW1_COMMAND_COMPONENT_ADAPTER, WASI_PREVIEW1_REACTOR_COMPONENT_ADAPTER,
};
use wasmtime::component::{types, Component, InstancePre, Linker};
use wasmtime::Store;
use wasmtime_wasi::{ResourceTable, WasiCtxBuilder};
use wasmtime_wasi::{WasiCtx, WasiView};
use wrpc_runtime_wasmtime::{link_instance, ServeExt as _, WrpcView};
use wrpc_transport::Invoke;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
enum Command {
    Run(RunArgs),
    Serve(ServeArgs),
}

/// Run a command component
#[derive(Parser, Debug)]
struct RunArgs {
    /// NATS address to use
    #[arg(short, long, default_value = wrpc_cli::nats::DEFAULT_URL)]
    nats: String,

    /// Target prefix to send invocations to
    target: String,

    /// Path or URL to Wasm command component
    workload: String,
}

/// Serve a reactor component
#[derive(Parser, Debug)]
struct ServeArgs {
    /// NATS address to use
    #[arg(short, long, default_value = wrpc_cli::nats::DEFAULT_URL)]
    nats: String,

    /// NATS queue group to use
    #[arg(short, long)]
    group: Option<String>,

    /// Target prefix to send invocations to
    target: String,

    /// Prefix to listen on
    prefix: String,

    /// Path or URL to Wasm command component
    workload: String,
}

pub enum Workload {
    Url(Url),
    Binary(Vec<u8>),
}

pub struct Ctx<C: Invoke> {
    pub table: ResourceTable,
    pub wasi: WasiCtx,
    pub wrpc: C,
}

impl<C: Invoke> WrpcView<C> for Ctx<C> {
    fn client(&self) -> &C {
        &self.wrpc
    }
}

impl<C: Invoke> WasiView for Ctx<C> {
    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.wasi
    }
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
}

#[instrument(level = "trace", skip(cx))]
async fn instantiate_pre<C>(
    adapter: &[u8],
    cx: C::Context,
    workload: &str,
) -> anyhow::Result<(InstancePre<Ctx<C>>, wasmtime::Engine)>
where
    C: Invoke,
    C::Context: Clone,
{
    let engine = wasmtime::Engine::new(
        wasmtime::Config::new()
            .async_support(true)
            .wasm_component_model(true),
    )
    .context("failed to initialize Wasmtime engine")?;

    let wasm = if workload.starts_with('.') {
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
            .adapter("wasi_snapshot_preview1", adapter)
            .context("failed to add WASI adapter")?
            .encode()
            .context("failed to encode a component")?
    } else {
        wasm
    };

    let component = Component::new(&engine, &wasm).context("failed to compile component")?;

    let mut linker = Linker::<Ctx<C>>::new(&engine);
    wasmtime_wasi::add_to_linker_async(&mut linker).context("failed to link WASI")?;

    let (resolve, world) =
        match wit_component::decode(&wasm).context("failed to decode WIT component")? {
            wit_component::DecodedWasm::Component(resolve, world) => (resolve, world),
            wit_component::DecodedWasm::WitPackages(..) => {
                bail!("binary-encoded WIT packages not currently supported")
            }
        };

    let wit_parser::World { imports, .. } = resolve
        .worlds
        .iter()
        .find_map(|(id, w)| (id == world).then_some(w))
        .context("component world missing")?;

    let ty = component.component_type();
    for (wk, _) in imports {
        let instance_name = resolve.name_world_key(wk);
        // Avoid polyfilling instances, for which static bindings are linked
        match instance_name.as_ref() {
            "wasi:cli/environment@0.2.0"
            | "wasi:cli/exit@0.2.0"
            | "wasi:cli/stderr@0.2.0"
            | "wasi:cli/stdin@0.2.0"
            | "wasi:cli/stdout@0.2.0"
            | "wasi:cli/terminal-input@0.2.0"
            | "wasi:cli/terminal-output@0.2.0"
            | "wasi:cli/terminal-stderr@0.2.0"
            | "wasi:cli/terminal-stdin@0.2.0"
            | "wasi:cli/terminal-stdout@0.2.0"
            | "wasi:clocks/monotonic-clock@0.2.0"
            | "wasi:clocks/wall-clock@0.2.0"
            | "wasi:filesystem/preopens@0.2.0"
            | "wasi:filesystem/types@0.2.0"
            | "wasi:http/incoming-handler@0.2.0"
            | "wasi:http/outgoing-handler@0.2.0"
            | "wasi:http/types@0.2.0"
            | "wasi:io/error@0.2.0"
            | "wasi:io/poll@0.2.0"
            | "wasi:io/streams@0.2.0"
            | "wasi:random/random@0.2.0"
            | "wasi:sockets/instance-network@0.2.0"
            | "wasi:sockets/network@0.2.0"
            | "wasi:sockets/tcp-create-socket@0.2.0"
            | "wasi:sockets/tcp@0.2.0"
            | "wasi:sockets/udp-create-socket@0.2.0"
            | "wasi:sockets/udp@0.2.0" => continue,
            _ => {}
        }
        let Some(types::ComponentItem::ComponentInstance(instance)) =
            ty.get_import(&engine, &instance_name)
        else {
            trace!(
                instance_name,
                "component does not import the parsed instance"
            );
            continue;
        };

        let mut linker = linker.root();
        let mut linker = match linker.instance(&instance_name) {
            Ok(linker) => linker,
            Err(err) => {
                error!(
                    ?err,
                    ?instance_name,
                    "failed to instantiate interface from root"
                );
                continue;
            }
        };
        if let Err(err) = link_instance(&engine, &mut linker, instance, instance_name, cx.clone()) {
            error!(?err, "failed to polyfill instance");
        }
    }

    let pre = linker
        .instantiate_pre(&component)
        .context("failed to pre-instantiate component")?;
    Ok((pre, engine))
}

fn new_store<C: Invoke>(engine: &wasmtime::Engine, wrpc: C, arg0: &str) -> wasmtime::Store<Ctx<C>> {
    Store::new(
        engine,
        Ctx {
            wasi: WasiCtxBuilder::new()
                .inherit_env()
                .inherit_stdio()
                .inherit_network()
                .args(&[arg0])
                .build(),
            table: ResourceTable::new(),
            wrpc,
        },
    )
}

#[instrument(level = "trace", ret)]
pub async fn handle_run(
    RunArgs {
        nats,
        target,
        ref workload,
    }: RunArgs,
) -> anyhow::Result<()> {
    let nats = wrpc_cli::nats::connect(nats)
        .await
        .context("failed to connect to NATS")?;

    let (pre, engine) =
        instantiate_pre(WASI_PREVIEW1_COMMAND_COMPONENT_ADAPTER, None, workload).await?;
    let mut store = new_store(
        &engine,
        wrpc_transport_nats::Client::new(nats, target, None),
        "command.wasm",
    );
    let (cmd, _) = wasmtime_wasi::bindings::Command::instantiate_pre(&mut store, &pre)
        .await
        .context("failed to instantiate `command`")?;
    cmd.wasi_cli_run()
        .call_run(&mut store)
        .await
        .context("failed to run component")?
        .map_err(|()| anyhow!("component failed"))
}

#[instrument(level = "trace", ret)]
pub async fn handle_serve(
    ServeArgs {
        nats,
        prefix,
        target,
        group,
        ref workload,
    }: ServeArgs,
) -> anyhow::Result<()> {
    let nats = wrpc_cli::nats::connect(nats)
        .await
        .context("failed to connect to NATS")?;
    let nats = Arc::new(nats);

    let (pre, engine) =
        instantiate_pre(WASI_PREVIEW1_REACTOR_COMPONENT_ADAPTER, None, workload).await?;

    let mut handlers = JoinSet::new();
    let clt = wrpc_transport_nats::Client::new(Arc::clone(&nats), target, None);
    let srv = wrpc_transport_nats::Client::new(nats, prefix, group.map(Arc::from));
    for (name, ty) in pre.component().component_type().exports(&engine) {
        match (name, ty) {
            (name, types::ComponentItem::ComponentFunc(ty)) => {
                let clt = clt.clone();
                let engine = engine.clone();
                info!(?name, "serving root function");
                let invocations = srv
                    .serve_function(
                        move || new_store(&engine, clt.clone(), "reactor.wasm"),
                        pre.clone(),
                        ty,
                        "",
                        name,
                    )
                    .await?;
                handlers.spawn(async move {
                    let mut invocations = pin!(invocations);
                    while let Some(invocation) = invocations.next().await {
                        match invocation {
                            Ok((headers, Ok(()))) => {
                                info!(?headers, "finished serving root function invocation");
                            }
                            Ok((headers, Err(err))) => {
                                warn!(?headers, ?err, "failed to serve root function invocation");
                            }
                            Err(err) => {
                                error!(?err, "failed to accept root function invocation");
                            }
                        }
                    }
                    ().in_current_span()
                });
            }
            (_, types::ComponentItem::CoreFunc(_)) => {
                bail!("serving root core function exports not supported yet")
            }
            (_, types::ComponentItem::Module(_)) => {
                bail!("serving root module exports not supported yet");
            }
            (_, types::ComponentItem::Component(_)) => {
                bail!("serving root component exports not supported yet");
            }
            (instance_name, types::ComponentItem::ComponentInstance(ty)) => {
                for (name, ty) in ty.exports(&engine) {
                    match ty {
                        types::ComponentItem::ComponentFunc(ty) => {
                            let clt = clt.clone();
                            let engine = engine.clone();
                            info!(?name, "serving instance function");
                            let invocations = srv
                                .serve_function(
                                    move || new_store(&engine, clt.clone(), "reactor.wasm"),
                                    pre.clone(),
                                    ty,
                                    instance_name,
                                    name,
                                )
                                .await?;
                            handlers.spawn(async move {
                                let mut invocations = pin!(invocations);
                                while let Some(invocation) = invocations.next().await {
                                    match invocation {
                                        Ok((headers, Ok(()))) => {
                                            info!(
                                                ?headers,
                                                "finished serving instance function invocation"
                                            );
                                        }
                                        Ok((headers, Err(err))) => {
                                            warn!(
                                                ?headers,
                                                ?err,
                                                "failed to serve instance function invocation"
                                            );
                                        }
                                        Err(err) => {
                                            error!(
                                                ?err,
                                                "failed to accept instance function invocation"
                                            );
                                        }
                                    }
                                }
                                ().in_current_span()
                            });
                        }
                        types::ComponentItem::CoreFunc(_) => {
                            bail!("serving instance core function exports not supported yet")
                        }
                        types::ComponentItem::Module(_) => {
                            bail!("serving instance module exports not supported yet")
                        }
                        types::ComponentItem::Component(_) => {
                            bail!("serving instance component exports not supported yet")
                        }
                        types::ComponentItem::ComponentInstance(_) => {
                            bail!("serving nested instance exports not supported yet")
                        }
                        types::ComponentItem::Type(_) => {}
                        types::ComponentItem::Resource(_) => {
                            bail!("serving instance resource exports not supported yet")
                        }
                    }
                }
            }
            (_, types::ComponentItem::Type(_)) => {}
            (_, types::ComponentItem::Resource(_)) => {
                bail!("serving root resource exports not supported yet")
            }
        }
    }
    while let Some(res) = handlers.join_next().await {
        if let Err(err) = res {
            error!(?err, "handler failed");
        }
    }
    Ok(())
}

#[instrument(level = "trace", ret)]
pub async fn run() -> anyhow::Result<()> {
    wrpc_cli::tracing::init();
    match Command::parse() {
        Command::Run(args) => handle_run(args).await,
        Command::Serve(args) => handle_serve(args).await,
    }
}
