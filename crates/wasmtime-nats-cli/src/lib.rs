use anyhow::{anyhow, bail, Context as _};
use clap::Parser;
use tokio::fs;
use tracing::instrument;
use url::Url;
use wasmcloud_component_adapters::WASI_PREVIEW1_COMMAND_COMPONENT_ADAPTER;
use wasmtime::{
    component::{Component, Linker},
    Store,
};
use wasmtime_wasi::bindings::Command;
use wasmtime_wasi::{ResourceTable, WasiCtxBuilder};

mod runtime;
use runtime::{polyfill, Ctx};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// NATS address to use
    #[arg(short, long, default_value = wrpc_cli::nats::DEFAULT_URL)]
    nats: Url,

    /// Prefix to listen on
    prefix: String,

    /// Path or URL to Wasm command component
    workload: String,
}

pub enum Workload {
    Url(Url),
    Binary(Vec<u8>),
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
    runtime::wasmtime_bindings::Interfaces::add_to_linker(&mut linker, |ctx| ctx)
        .context("failed to link `wrpc:runtime/interfaces` interface")?;

    wasmtime_wasi::add_to_linker_async(&mut linker).context("failed to link WASI")?;

    let (resolve, world) =
        match wit_component::decode(&wasm).context("failed to decode WIT component")? {
            wit_component::DecodedWasm::Component(resolve, world) => (resolve, world),
            wit_component::DecodedWasm::WitPackage(..) => {
                bail!("binary-encoded WIT packages not currently supported")
            }
        };

    let wit_parser::World { imports, .. } = resolve
        .worlds
        .iter()
        .find_map(|(id, w)| (id == world).then_some(w))
        .context("component world missing")?;

    polyfill(
        &resolve,
        imports,
        &engine,
        &component.component_type(),
        &mut linker,
    );

    let pre = linker
        .instantiate_pre(&component)
        .context("failed to pre-instantiate component")?;

    let mut store = Store::new(
        &engine,
        Ctx {
            ctx: WasiCtxBuilder::new()
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
