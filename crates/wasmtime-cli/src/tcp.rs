use anyhow::Context as _;
use clap::Parser;
use tracing::instrument;

pub const DEFAULT_ADDR: &str = "[::1]:7761";

/// TCP transport
#[derive(Parser, Debug)]
pub enum Command {
    Serve(ServeArgs),
}

/// Serve a reactor component
#[derive(Parser, Debug)]
pub struct ServeArgs {
    /// Limit on how long an accepted connection may take to send its invocation header
    #[arg(long, default_value = crate::DEFAULT_TIMEOUT)]
    timeout: humantime::Duration,

    /// Address to listen for export invocations on
    #[arg(long, default_value = DEFAULT_ADDR)]
    export: String,

    /// Environment variable to set for the component, in `NAME=VALUE` form
    #[arg(long = "env", value_name = "NAME=VALUE", value_parser = parse_env)]
    envs: Vec<(Box<str>, Box<str>)>,

    /// Path or URL to Wasm reactor component
    workload: Box<str>,
}

fn parse_env(s: &str) -> anyhow::Result<(Box<str>, Box<str>)> {
    let (key, val) = s
        .split_once('=')
        .with_context(|| "invalid `--env` value `{s}`: expected `NAME=VALUE`")?;
    Ok((key.into(), val.into()))
}

#[instrument(level = "trace", ret(level = "trace"))]
pub async fn handle_serve(
    ServeArgs {
        timeout,
        export,
        envs,
        ref workload,
    }: ServeArgs,
) -> anyhow::Result<()> {
    let lis = tokio::net::TcpListener::bind(&export)
        .await
        .with_context(|| format!("failed to bind TCP listener on `{export}`"))?;
    crate::serve(wrpc_transport::tcp::incoming(lis), *timeout, envs, workload).await
}

#[instrument(level = "trace", ret(level = "trace"))]
pub async fn run(cmd: Command) -> anyhow::Result<()> {
    match cmd {
        Command::Serve(args) => handle_serve(args).await,
    }
}
