use core::net::{Ipv6Addr, SocketAddr};

use anyhow::Context as _;
use clap::Parser;
use quinn::{ClientConfig, Endpoint};
use tracing::instrument;

pub const DEFAULT_ADDR: &str = "[::1]:4433";

/// QUIC transport
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
    import_addr: SocketAddr,

    /// Server name to use for import connection
    #[arg(long)]
    import_san: String,

    /// Path or URL to Wasm command component
    workload: String,
}

#[instrument(level = "trace", ret(level = "trace"))]
pub async fn handle_run(
    RunArgs {
        timeout,
        import_addr,
        import_san,
        ref workload,
    }: RunArgs,
) -> anyhow::Result<()> {
    let mut ep = Endpoint::client((Ipv6Addr::UNSPECIFIED, 0).into())
        .context("failed to create QUIC endpoint")?;
    // TODO: Allow TLS configuration via runtime flags
    // TODO: Support WebPKI
    ep.set_default_client_config(ClientConfig::with_platform_verifier());
    let conn = ep
        .connect(import_addr, &import_san)
        .context("failed to connect using QUIC")?;
    let conn = conn.await.context("failed to establish QUIC connection")?;
    crate::handle_run(
        wrpc_transport_quic::Client::from(conn),
        (),
        *timeout,
        workload,
    )
    .await
}

#[instrument(level = "trace", skip_all, ret(level = "trace"))]
pub async fn run(cmd: Command) -> anyhow::Result<()> {
    match cmd {
        Command::Run(args) => handle_run(args).await,
        // TODO: Implement serving
    }
}
