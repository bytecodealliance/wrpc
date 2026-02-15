use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;
use tracing::instrument;

/// Zenoh transport
#[derive(Parser, Debug)]
pub enum Command {
    Run(RunArgs),
    Serve(ServeArgs),
}

/// Run a command component
#[derive(Parser, Debug)]
pub struct RunArgs {
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
    /// Invocation timeout
    #[arg(long, default_value = crate::DEFAULT_TIMEOUT)]
    timeout: humantime::Duration,

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
        timeout,
        import,
        ref workload,
    }: RunArgs,
) -> anyhow::Result<()> {
    let zenoh = wrpc_cli::zenoh::connect()
        .await
        .context("failed to connect to zenoh")?;
    let zenoh_client = wrpc_transport_zenoh::Client::new(zenoh, import)
        .await
        .context("failed to construct zenoh transport")?;
    let res = crate::handle_run(zenoh_client, (), *timeout, workload).await;
    //println!("foooooo run");
    res
}

#[instrument(level = "trace", ret(level = "trace"))]
pub async fn handle_serve(
    ServeArgs {
        timeout,
        export,
        import,
        ref workload,
    }: ServeArgs,
) -> anyhow::Result<()> {
    let zenoh = wrpc_cli::zenoh::connect()
        .await
        .context("failed to connect to zenoh")?;
    let zenoh = Arc::new(zenoh);
    let exports = wrpc_transport_zenoh::Client::new(zenoh.clone(), export)
        .await
        .context("failed to construct zenoh transport export client")?;
    let imports = wrpc_transport_zenoh::Client::new(zenoh.clone(), import)
        .await
        .context("failed to construct zenoh transport import client")?;
        //println!("foooooo serve");
    // future::pending::<()>().await;
    // res
    crate::handle_serve(exports, imports, (), *timeout, workload).await
}

#[instrument(level = "trace", ret(level = "trace"))]
pub async fn run(cmd: Command) -> anyhow::Result<()> {
    match cmd {
        Command::Run(args) => handle_run(args).await,
        Command::Serve(args) => handle_serve(args).await,
    }
}
