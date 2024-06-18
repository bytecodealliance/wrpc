#![allow(unused)]

use core::future::Future;

use std::net::Ipv6Addr;
use std::process::ExitStatus;
use std::sync::Arc;

use anyhow::Context;
use quinn::crypto::rustls::QuicClientConfig;
use quinn::{ClientConfig, EndpointConfig, ServerConfig, TokioRuntime};
use rcgen::{generate_simple_self_signed, CertifiedKey};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use rustls::version::TLS13;
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot, OnceCell};
use tokio::task::JoinHandle;
use tokio::{select, spawn};

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
    u16,
    async_nats::Client,
    JoinHandle<anyhow::Result<ExitStatus>>,
    oneshot::Sender<()>,
)> {
    let port = free_port().await?;
    let (server, stop_tx) =
        spawn_server(Command::new("nats-server").args(["-V", "-T=false", "-p", &port.to_string()]))
            .await
            .context("failed to start NATS.io server")?;

    let client = wrpc_cli::nats::connect(format!("nats://localhost:{port}"))
        .await
        .context("failed to connect to NATS.io server")?;
    Ok((port, client, server, stop_tx))
}

pub async fn with_nats<T, Fut>(f: impl FnOnce(u16, async_nats::Client) -> Fut) -> anyhow::Result<T>
where
    Fut: Future<Output = anyhow::Result<T>>,
{
    let (port, nats_client, nats_server, stop_tx) = start_nats()
        .await
        .context("failed to start NATS.io server")?;
    let res = f(port, nats_client).await.context("closure failed")?;
    stop_tx.send(()).expect("failed to stop NATS.io server");
    nats_server
        .await
        .context("failed to await NATS.io server stop")?
        .context("NATS.io server failed to stop")?;
    Ok(res)
}

pub async fn with_quic<T, Fut>(
    names: &[&str],
    f: impl FnOnce(u16, quinn::Endpoint, quinn::Endpoint) -> Fut,
) -> anyhow::Result<T>
where
    Fut: Future<Output = anyhow::Result<T>>,
{
    let CertifiedKey {
        cert: srv_crt,
        key_pair: srv_key,
    } = generate_simple_self_signed(
        names
            .iter()
            .map(|name| format!("{name}.server.wrpc"))
            .collect::<Vec<_>>(),
    )
    .context("failed to generate server certificate")?;
    let CertifiedKey {
        cert: clt_crt,
        key_pair: clt_key,
    } = generate_simple_self_signed(
        names
            .iter()
            .map(|name| format!("{name}.client.wrpc"))
            .collect::<Vec<_>>(),
    )
    .context("failed to generate client certificate")?;
    let srv_crt = CertificateDer::from(srv_crt);

    let mut ca = rustls::RootCertStore::empty();
    ca.add(srv_crt.clone())?;
    let clt_cnf = rustls::ClientConfig::builder_with_protocol_versions(&[&TLS13])
        .with_root_certificates(ca)
        .with_client_auth_cert(
            vec![clt_crt.into()],
            PrivatePkcs8KeyDer::from(clt_key.serialize_der()).into(),
        )
        .context("failed to create client config")?;
    let clt_cnf: QuicClientConfig = clt_cnf
        .try_into()
        .context("failed to convert rustls client config to QUIC client config")?;
    let srv_cnf = ServerConfig::with_single_cert(
        vec![srv_crt],
        PrivatePkcs8KeyDer::from(srv_key.serialize_der()).into(),
    )
    .expect("failed to create server config");

    let mut clt_ep = quinn::Endpoint::client((Ipv6Addr::LOCALHOST, 0).into())
        .context("failed to create client endpoint")?;
    clt_ep.set_default_client_config(ClientConfig::new(Arc::new(clt_cnf)));

    let srv_sock = std::net::UdpSocket::bind((Ipv6Addr::LOCALHOST, 0))
        .context("failed to open a UDP socket")?;
    let srv_addr = srv_sock
        .local_addr()
        .context("failed to query server address")?;
    let srv_ep = quinn::Endpoint::new(
        EndpointConfig::default(),
        Some(srv_cnf),
        srv_sock,
        Arc::new(TokioRuntime),
    )
    .context("failed to create server endpoint")?;
    f(srv_addr.port(), clt_ep, srv_ep)
        .await
        .context("closure failed")
}
