use anyhow::{anyhow, bail, Context as _};
use clap::Parser;
use tokio::fs;
use tracing::{error, instrument, trace};
use url::Url;
use wasmcloud_component_adapters::WASI_PREVIEW1_COMMAND_COMPONENT_ADAPTER;
use wasmtime::component::{types, Component, Linker};
use wasmtime::Store;
use wasmtime_wasi::{bindings::Command, WasiCtx, WasiView};
use wasmtime_wasi::{ResourceTable, WasiCtxBuilder};
use wrpc_runtime_wasmtime::{link_instance, WrpcView};
use wrpc_transport::Invoke;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// NATS address to use
    #[arg(short, long, default_value = wrpc_cli::nats::DEFAULT_URL)]
    nats: String,

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

#[instrument(level = "trace", ret)]
pub async fn run() -> anyhow::Result<()> {
    wrpc_cli::tracing::init();

    let Args {
        nats,
        prefix,
        workload,
    } = Args::parse();
    let nats = wrpc_cli::nats::connect(nats)
        .await
        .context("failed to connect to NATS")?;

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
        Url::parse(&workload)
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
                "wasi_snapshot_preview1",
                WASI_PREVIEW1_COMMAND_COMPONENT_ADAPTER,
            )
            .context("failed to add WASI adapter")?
            .encode()
            .context("failed to encode a component")?
    } else {
        wasm
    };

    let component = Component::new(&engine, &wasm).context("failed to compile component")?;

    let mut linker = Linker::<Ctx<wrpc_transport_nats::Client>>::new(&engine);
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
            | "wasi:keyvalue/store@0.2.0-draft"
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
        if let Err(err) = link_instance(&engine, &mut linker, instance, instance_name, None) {
            error!(?err, "failed to polyfill instance");
        }
    }

    let pre = linker
        .instantiate_pre(&component)
        .context("failed to pre-instantiate component")?;

    let mut store = Store::new(
        &engine,
        Ctx {
            wasi: WasiCtxBuilder::new()
                .inherit_env()
                .inherit_stdio()
                .inherit_network()
                .args(&["main.wasm"])
                .build(),
            table: ResourceTable::new(),
            wrpc: wrpc_transport_nats::Client::new(nats, prefix),
        },
    );
    let (cmd, _) = Command::instantiate_pre(&mut store, &pre)
        .await
        .context("failed to instantiate `command`")?;
    cmd.wasi_cli_run()
        .call_run(&mut store)
        .await
        .context("failed to run component")?
        .map_err(|()| anyhow!("component failed"))
}
