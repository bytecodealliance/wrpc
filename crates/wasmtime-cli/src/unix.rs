use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;
use tracing::{error, instrument};

/// Unix Domain Socket transport
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

    /// Path to send import invocations to
    #[arg(long)]
    import: PathBuf,

    /// Path or URL to Wasm command component
    workload: String,
}

/// Serve a reactor component
#[derive(Parser, Debug)]
pub struct ServeArgs {
    /// Invocation timeout
    #[arg(long, default_value = crate::DEFAULT_TIMEOUT)]
    timeout: humantime::Duration,

    /// Path to send import invocations to
    #[arg(long)]
    import: PathBuf,

    /// Path to listen for export invocations on
    #[arg(long)]
    export: PathBuf,

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
        wrpc_transport::unix::Client::from(import),
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
    let lis = tokio::net::UnixListener::bind(&export).with_context(|| {
        format!(
            "failed to bind Unix socket listener on `{}`",
            export.display()
        )
    })?;
    let srv = Arc::new(wrpc_transport::Server::default());
    let accept = tokio::spawn({
        let srv = Arc::clone(&srv);
        async move {
            loop {
                if let Err(err) = srv.accept(&lis).await {
                    error!(?err, "failed to accept Unix socket connection");
                }
            }
        }
    });
    let res = crate::handle_serve(
        srv.as_ref(),
        wrpc_transport::unix::Client::from(import),
        (),
        *timeout,
        workload,
    )
    .await;
    accept.abort();
    res
}

#[instrument(level = "trace", skip_all, ret(level = "trace"))]
pub async fn run(cmd: Command) -> anyhow::Result<()> {
    match cmd {
        Command::Run(args) => handle_run(args).await,
        Command::Serve(args) => handle_serve(args).await,
    }
}
