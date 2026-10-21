use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;
use tokio::io::BufReader;
use tokio::select;
use tokio::task::JoinSet;
use tokio::time;
use tracing::{error, instrument, warn};

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
    /// Invocation timeout, also limiting how long an accepted connection may take to send its
    /// invocation header
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
    let timeout = *timeout;
    let mut handlers = crate::serve(
        srv.as_ref(),
        wrpc_transport::tcp::Client::from(import),
        (),
        timeout,
        envs,
        workload,
    )
    .await?;
    let accept = tokio::spawn({
        let srv = Arc::clone(&srv);
        async move {
            let mut conns = JoinSet::default();
            loop {
                select! {
                    res = lis.accept() => match res {
                        Ok((stream, addr)) => {
                            if let Err(err) = stream.set_nodelay(true) {
                                warn!(?err, "failed to set TCP_NODELAY");
                            }
                            let (rx, tx) = stream.into_split();
                            let rx = BufReader::new(rx);
                            let srv = Arc::clone(&srv);
                            conns.spawn(async move {
                                match time::timeout(timeout, srv.accept(addr, tx, rx)).await {
                                    Ok(Ok(())) => {}
                                    Ok(Err(err)) => error!(?err, "failed to serve TCP connection"),
                                    Err(..) => error!(?addr, "timed out accepting TCP connection"),
                                }
                            });
                        }
                        Err(err) => error!(?err, "failed to accept TCP connection"),
                    },
                    Some(res) = conns.join_next() => {
                        if let Err(err) = res {
                            error!(?err, "failed to join TCP connection task");
                        }
                    }
                }
            }
        }
    });
    while let Some(res) = handlers.join_next().await {
        if let Err(err) = res {
            error!(?err, "handler failed");
        }
    }
    accept.abort();
    Ok(())
}

#[instrument(level = "trace", ret(level = "trace"))]
pub async fn run(cmd: Command) -> anyhow::Result<()> {
    match cmd {
        Command::Run(args) => handle_run(args).await,
        Command::Serve(args) => handle_serve(args).await,
    }
}
