use anyhow::Context as _;
use clap::Parser;
use tracing::instrument;
use wtransport::{ClientConfig, Endpoint};

pub const DEFAULT_ADDR: &str = "https://localhost:4433";

/// WebTransport transport
#[derive(Parser, Debug)]
pub enum Command {
    Run(RunArgs),
}

/// Run a command component
#[derive(Parser, Debug)]
pub struct RunArgs {
    /// Invocation timeout
    #[arg(long, default_value = crate::DEFAULT_TIMEOUT)]
    timeout: humantime::Duration,

    /// Address to send import invocations to
    #[arg(long, default_value = DEFAULT_ADDR)]
    import: String,

    /// Path or URL to Wasm command component
    workload: String,
}

#[instrument(level = "trace", ret(level = "trace"))]
pub async fn handle_run(
    RunArgs {
        timeout,
        import,
        ref workload,
    }: RunArgs,
) -> anyhow::Result<()> {
    let ep = Endpoint::client(ClientConfig::default())
        .context("failed to create WebTransport endpoint")?;
    // TODO: Allow TLS configuration via runtime flags
    // TODO: Support WebPKI
    let conn = ep
        .connect(import)
        .await
        .context("failed to connect using WebTransport")?;
    crate::handle_run(
        wrpc_transport_web::Client::from(conn),
        (),
        *timeout,
        workload,
    )
    .await
}

#[instrument(level = "trace", ret(level = "trace"))]
pub async fn run(cmd: Command) -> anyhow::Result<()> {
    match cmd {
        Command::Run(args) => handle_run(args).await,
        // TODO: Implement serving
    }
}
