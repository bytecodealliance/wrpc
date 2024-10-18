use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;
use tracing::{error, instrument};

pub const DEFAULT_ADDR: &str = "[::1]:7761";

/// TCP transport
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

    /// Address to send import invocations to
    #[arg(long, default_value = DEFAULT_ADDR)]
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

    /// Address to send import invocations to
    #[arg(long, default_value = DEFAULT_ADDR)]
    import: String,

    /// Address to listen for export invocations on
    #[arg(long, default_value = DEFAULT_ADDR)]
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
    crate::handle_run(
        wrpc_transport::tcp::Client::from(import),
        (),
        *timeout,
        workload,
    )
    .await
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
    let lis = tokio::net::TcpListener::bind(&export)
        .await
        .with_context(|| format!("failed to bind TCP listener on `{export}`"))?;
    let srv = Arc::new(wrpc_transport::Server::default());
    let accept = tokio::spawn({
        let srv = Arc::clone(&srv);
        async move {
            loop {
                if let Err(err) = srv.accept(&lis).await {
                    error!(?err, "failed to accept TCP connection");
                }
            }
        }
    });
    let res = crate::handle_serve(
        srv.as_ref(),
        wrpc_transport::tcp::Client::from(import),
        (),
        *timeout,
        workload,
    )
    .await;
    accept.abort();
    res
}

#[instrument(level = "trace", ret(level = "trace"))]
pub async fn run(cmd: Command) -> anyhow::Result<()> {
    match cmd {
        Command::Run(args) => handle_run(args).await,
        Command::Serve(args) => handle_serve(args).await,
    }
}
