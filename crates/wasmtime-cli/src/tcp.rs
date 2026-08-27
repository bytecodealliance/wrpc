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

    /// Environment variable to set for the component, in `NAME=VALUE` form
    #[arg(long = "env", value_name = "NAME=VALUE", value_parser = parse_env)]
    envs: Vec<(Box<str>, Box<str>)>,

    /// Path or URL to Wasm command component
    workload: Box<str>,
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

    /// Environment variable to set for the component, in `NAME=VALUE` form
    #[arg(long = "env", value_name = "NAME=VALUE", value_parser = parse_env)]
    envs: Vec<(Box<str>, Box<str>)>,

    /// Path or URL to Wasm command component
    workload: Box<str>,
}

fn parse_env(s: &str) -> anyhow::Result<(Box<str>, Box<str>)> {
    let (key, val) = s
        .split_once('=')
        .with_context(|| "invalid `--env` value `{s}`: expected `NAME=VALUE`")?;
    Ok((key.into(), val.into()))
}

#[instrument(level = "trace", ret(level = "trace"))]
pub async fn handle_run(
    RunArgs {
        timeout,
        import,
        envs,
        ref workload,
    }: RunArgs,
) -> anyhow::Result<()> {
    crate::handle_run(
        wrpc_transport::tcp::Client::from(import),
        (),
        *timeout,
        envs,
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
        envs,
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
                match lis.accept().await {
                    Ok((stream, addr)) => {
                        let (rx, tx) = stream.into_split();
                        if let Err(err) = srv.accept(addr, tx, rx).await {
                            error!(?err, "failed to serve TCP connection");
                        }
                    }
                    Err(err) => error!(?err, "failed to accept TCP connection"),
                }
            }
        }
    });
    let res = crate::handle_serve(
        srv.as_ref(),
        wrpc_transport::tcp::Client::from(import),
        (),
        *timeout,
        envs,
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
