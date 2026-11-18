use anyhow::Context as _;
use clap::Parser;
use tracing::instrument;

pub const DEFAULT_ADDR: &str = "[::1]:7761";

const DEFAULT_TIMEOUT: &str = "10s";

/// TCP transport
#[derive(Parser, Debug)]
pub enum Command {
    Serve(ServeArgs),
}

/// Serve a reactor component
#[derive(Parser, Debug)]
pub struct ServeArgs {
    /// Limit on how long a connection may stay idle waiting for its next invocation header
    #[arg(long, default_value = DEFAULT_TIMEOUT)]
    timeout: humantime::Duration,

    /// Limit on the length of a string in bytes and of a list in elements decoded from parameters
    #[arg(long, default_value_t = wrpc_wasmtime::DEFAULT_MAX_LEN)]
    max_len: u32,

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
        .with_context(|| format!("invalid `--env` value `{s}`: expected `NAME=VALUE`"))?;
    Ok((key.into(), val.into()))
}

#[instrument(level = "trace", ret(level = "trace"))]
pub async fn handle_serve(
    ServeArgs {
        timeout,
        max_len,
        export,
        envs,
        ref workload,
    }: ServeArgs,
) -> anyhow::Result<()> {
    let lis = tokio::net::TcpListener::bind(&export)
        .await
        .with_context(|| format!("failed to bind TCP listener on `{export}`"))?;
    crate::serve(lis, *timeout, max_len, envs, workload).await
}

#[instrument(level = "trace", ret(level = "trace"))]
pub async fn run(cmd: Command) -> anyhow::Result<()> {
    match cmd {
        Command::Serve(args) => handle_serve(args).await,
    }
}
