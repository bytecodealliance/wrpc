use core::net::Ipv6Addr;

use std::process::ExitStatus;

use anyhow::Context;
use rcgen::{generate_simple_self_signed, CertifiedKey};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use rustls::version::TLS13;
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::{select, spawn};

pub async fn free_port() -> anyhow::Result<u16> {
    TcpListener::bind((Ipv6Addr::LOCALHOST, 0))
        .await
        .context("failed to start TCP listener")?
        .local_addr()
        .context("failed to query listener local address")
        .map(|v| v.port())
}

pub async fn spawn_server(
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

pub fn cert_pair() -> anyhow::Result<(rustls::ServerConfig, rustls::ClientConfig)> {
    let CertifiedKey {
        cert: srv_crt,
        key_pair: srv_key,
    } = generate_simple_self_signed([
        "127.0.0.1".to_string(),
        "::1".to_string(),
        "localhost".to_string(),
    ])
    .context("failed to generate server certificate")?;
    let CertifiedKey {
        cert: clt_crt,
        key_pair: clt_key,
    } = generate_simple_self_signed(["client.wrpc".to_string()])
        .context("failed to generate client certificate")?;
    let srv_crt = CertificateDer::from(srv_crt);

    let mut ca = RootCertStore::empty();
    ca.add(srv_crt.clone())?;
    let clt_cnf = ClientConfig::builder_with_protocol_versions(&[&TLS13])
        .with_root_certificates(ca)
        .with_client_auth_cert(
            vec![clt_crt.into()],
            PrivatePkcs8KeyDer::from(clt_key.serialize_der()).into(),
        )
        .context("failed to create client config")?;
    let srv_cnf = ServerConfig::builder_with_protocol_versions(&[&TLS13])
        .with_no_client_auth() // TODO: verify client cert
        .with_single_cert(
            vec![srv_crt],
            PrivatePkcs8KeyDer::from(srv_key.serialize_der()).into(),
        )
        .context("failed to create server config")?;
    Ok((srv_cnf, clt_cnf))
}

#[cfg(feature = "nats")]
pub async fn start_nats() -> anyhow::Result<(
    u16,
    async_nats::Client,
    JoinHandle<anyhow::Result<ExitStatus>>,
    oneshot::Sender<()>,
)> {
    let port = free_port().await?;
    let (server, stop_tx) =
        spawn_server(Command::new("nats-server").args(["-T=false", "-p", &port.to_string()]))
            .await
            .context("failed to start NATS.io server")?;

    let client = wrpc_cli::nats::connect(format!("nats://localhost:{port}"))
        .await
        .context("failed to connect to NATS.io server")?;
    Ok((port, client, server, stop_tx))
}

#[cfg(feature = "nats")]
pub async fn with_nats<T, Fut>(f: impl FnOnce(u16, async_nats::Client) -> Fut) -> anyhow::Result<T>
where
    Fut: core::future::Future<Output = anyhow::Result<T>>,
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

#[cfg(feature = "quic")]
pub async fn with_quic_endpoints<T, Fut>(
    f: impl FnOnce(core::net::SocketAddr, quinn::Endpoint, quinn::Endpoint) -> Fut,
) -> anyhow::Result<T>
where
    Fut: core::future::Future<Output = anyhow::Result<T>>,
{
    use std::sync::Arc;

    use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
    use quinn::{ClientConfig, ServerConfig};

    let mut clt_ep = quinn::Endpoint::client((Ipv6Addr::LOCALHOST, 0).into())
        .context("failed to create client endpoint")?;

    let (srv_cnf, clt_cnf) = cert_pair().context("failed to generate certificates")?;

    let clt_cnf: QuicClientConfig = clt_cnf
        .try_into()
        .context("failed to convert rustls client config to QUIC client config")?;
    let srv_cnf: QuicServerConfig = srv_cnf
        .try_into()
        .context("failed to convert rustls server config to QUIC server config")?;

    clt_ep.set_default_client_config(ClientConfig::new(Arc::new(clt_cnf)));
    let srv_ep = quinn::Endpoint::server(
        ServerConfig::with_crypto(Arc::new(srv_cnf)),
        (Ipv6Addr::LOCALHOST, 0).into(),
    )
    .context("failed to create server endpoint")?;
    let srv_addr = srv_ep
        .local_addr()
        .context("failed to query server address")?;

    f(srv_addr, clt_ep, srv_ep).await.context("closure failed")
}

#[cfg(feature = "quic")]
pub async fn with_quic<T, Fut>(
    f: impl FnOnce(quinn::Connection, quinn::Connection) -> Fut,
) -> anyhow::Result<T>
where
    Fut: core::future::Future<Output = anyhow::Result<T>>,
{
    with_quic_endpoints(|addr, clt, srv| async move {
        let (clt, srv) = tokio::try_join!(
            async move {
                let conn = clt
                    .connect(addr, "::1")
                    .context("failed to connect to server")?;
                conn.await.context("failed to establish client connection")
            },
            async move {
                let conn = srv.accept().await.context("failed to accept connection")?;
                conn.await.context("failed to establish server connection")
            }
        )?;
        f(clt, srv).await.context("closure failed")
    })
    .await
}

#[cfg(feature = "web-transport")]
pub async fn with_web_transport<T, Fut>(
    f: impl FnOnce(wtransport::Connection, wtransport::Connection) -> Fut,
) -> anyhow::Result<T>
where
    Fut: core::future::Future<Output = anyhow::Result<T>>,
{
    use wtransport::Endpoint;

    let (srv_cnf, clt_cnf) = cert_pair().context("failed to generate certificates")?;

    let srv = Endpoint::server(
        wtransport::ServerConfig::builder()
            .with_bind_default(0)
            .with_custom_tls(srv_cnf)
            .build(),
    )
    .context("failed to build server endpoint")?;
    let clt = Endpoint::client(
        wtransport::ClientConfig::builder()
            .with_bind_default()
            .with_custom_tls(clt_cnf)
            .build(),
    )
    .context("failed to create client endpoint")?;
    let addr = srv.local_addr().context("failed to query server address")?;
    let (clt, srv) = tokio::try_join!(
        async move {
            clt.connect(format!("https://localhost:{}", addr.port()))
                .await
                .context("failed to connect to server")
        },
        async move {
            let req = srv
                .accept()
                .await
                .await
                .context("failed to receive session request")?;
            req.accept()
                .await
                .context("failed to accept client connection")
        }
    )?;
    f(clt, srv).await.context("closure failed")
}

#[cfg(feature = "memory")]
pub async fn with_memory<T, Fut>(
    f: impl FnOnce(wrpc_transport_memory::Client, wrpc_transport_memory::Server) -> Fut,
) -> anyhow::Result<T>
where
    Fut: core::future::Future<Output = anyhow::Result<T>>,
{
    let (clt, srv) = wrpc_transport_memory::new_memory_transport();
    f(clt, srv).await.context("closure failed")
}
