#![allow(unused)]

use core::future::Future;

use std::net::Ipv6Addr;
use std::process::ExitStatus;

use anyhow::Context;
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot, OnceCell};
use tokio::task::JoinHandle;
use tokio::{select, spawn};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

static INIT: OnceCell<()> = OnceCell::const_new();

async fn init_log() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().compact().without_time())
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init()
}

pub async fn init() {
    INIT.get_or_init(init_log).await;
}

async fn free_port() -> anyhow::Result<u16> {
    TcpListener::bind((Ipv6Addr::LOCALHOST, 0))
        .await
        .context("failed to start TCP listener")?
        .local_addr()
        .context("failed to query listener local address")
        .map(|v| v.port())
}

async fn spawn_server(
    cmd: &mut Command,
) -> anyhow::Result<(JoinHandle<anyhow::Result<ExitStatus>>, oneshot::Sender<()>)> {
    let mut child = cmd
        .kill_on_drop(true)
        .spawn()
        .context("failed to spawn child")?;
    let (stop_tx, stop_rx) = oneshot::channel();
    let child = spawn(async move {
        select!(
            res = stop_rx => {
                res.context("failed to wait for shutdown")?;
                child.kill().await.context("failed to kill child")?;
                child.wait().await
            }
            status = child.wait() => {
                status
            }
        )
        .context("failed to wait for child")
    });
    Ok((child, stop_tx))
}

pub async fn start_nats() -> anyhow::Result<(
    async_nats::Client,
    JoinHandle<anyhow::Result<ExitStatus>>,
    oneshot::Sender<()>,
)> {
    let port = free_port().await?;
    let (server, stop_tx) =
        spawn_server(Command::new("nats-server").args(["-V", "-T=false", "-p", &port.to_string()]))
            .await
            .context("failed to start NATS.io server")?;

    let (conn_tx, mut conn_rx) = mpsc::channel(1);
    let client = async_nats::connect_with_options(
        format!("nats://localhost:{port}"),
        async_nats::ConnectOptions::new()
            .retry_on_initial_connect()
            .event_callback(move |event| {
                let conn_tx = conn_tx.clone();
                async move {
                    if let async_nats::Event::Connected = event {
                        conn_tx
                            .send(())
                            .await
                            .expect("failed to send NATS.io server connection notification");
                    }
                }
            }),
    )
    .await
    .context("failed to connect to NATS.io server")?;
    conn_rx
        .recv()
        .await
        .context("failed to await NATS.io server connection to be established")?;
    Ok((client, server, stop_tx))
}

pub async fn with_nats<T, Fut>(f: impl FnOnce(async_nats::Client) -> Fut) -> anyhow::Result<T>
where
    Fut: Future<Output = anyhow::Result<T>>,
{
    let (nats_client, nats_server, stop_tx) = start_nats()
        .await
        .context("failed to start NATS.io server")?;
    let res = f(nats_client).await.context("closure failed")?;
    stop_tx.send(()).expect("failed to stop NATS.io server");
    nats_server
        .await
        .context("failed to await NATS.io server stop")?
        .context("NATS.io server failed to stop")?;
    Ok(res)
}
