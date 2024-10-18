use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;
use tracing::instrument;

/// NATS transport
#[derive(Parser, Debug)]
pub enum Command {
    Run(RunArgs),
    Serve(ServeArgs),
}

/// Run a command component
#[derive(Parser, Debug)]
pub struct RunArgs {
    /// NATS address to use
    #[arg(short, long, default_value = wrpc_cli::nats::DEFAULT_URL)]
    nats: String,

    /// Invocation timeout
    #[arg(long, default_value = crate::DEFAULT_TIMEOUT)]
    timeout: humantime::Duration,

    /// Prefix to send import invocations to
    #[arg(long, default_value = "")]
    import: String,

    /// Path or URL to Wasm command component
    workload: String,
}

/// Serve a reactor component
#[derive(Parser, Debug)]
pub struct ServeArgs {
    /// NATS address to use
    #[arg(short, long, default_value = wrpc_cli::nats::DEFAULT_URL)]
    nats: String,

    /// Invocation timeout
    #[arg(long, default_value = crate::DEFAULT_TIMEOUT)]
    timeout: humantime::Duration,

    /// NATS queue group to use
    #[arg(short, long)]
    group: Option<String>,

    /// Prefix to send import invocations to
    #[arg(long, default_value = "")]
    import: String,

    /// Prefix to listen for export invocations on
    #[arg(long, default_value = "")]
    export: String,

    /// Path or URL to Wasm command component
    workload: String,
}

#[instrument(level = "trace", ret(level = "trace"))]
pub async fn handle_run(
    RunArgs {
        nats,
        timeout,
        import,
        ref workload,
    }: RunArgs,
) -> anyhow::Result<()> {
    let nats = wrpc_cli::nats::connect(nats)
        .await
        .context("failed to connect to NATS")?;
    crate::handle_run(
        wrpc_transport_nats::Client::new(nats, import, None),
        None,
        *timeout,
        workload,
    )
    .await
}

#[instrument(level = "trace", ret(level = "trace"))]
pub async fn handle_serve(
    ServeArgs {
        nats,
        timeout,
        export,
        import,
        group,
        ref workload,
    }: ServeArgs,
) -> anyhow::Result<()> {
    let nats = wrpc_cli::nats::connect(nats)
        .await
        .context("failed to connect to NATS")?;
    let nats = Arc::new(nats);
    crate::handle_serve(
        wrpc_transport_nats::Client::new(Arc::clone(&nats), export, group.map(Arc::from)),
        wrpc_transport_nats::Client::new(nats, import, None),
        None,
        *timeout,
        workload,
    )
    .await
}

#[instrument(level = "trace", ret(level = "trace"))]
pub async fn run(cmd: Command) -> anyhow::Result<()> {
    match cmd {
        Command::Run(args) => handle_run(args).await,
        Command::Serve(args) => handle_serve(args).await,
    }
}
