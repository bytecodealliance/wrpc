use core::net::SocketAddr;
use core::pin::pin;
use core::time::Duration;

use std::sync::Arc;

use anyhow::{Context as _, bail};
use clap::Parser;
use rcgen::{CertifiedKey, generate_simple_self_signed};
use rustls::pki_types::PrivatePkcs8KeyDer;
use rustls::version::TLS13;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::task::JoinSet;
use tokio::{select, signal, spawn};
use tracing::{debug, error, info, warn};
use web_transport_quinn::quinn::crypto::rustls::QuicServerConfig;
use web_transport_quinn::{Request, quinn};
use wrpc_transport::{Header, Listener};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Address to serve `wasi:keyvalue` on
    #[arg(default_value = "[::1]:4433")]
    addr: SocketAddr,
}

async fn serve(
    handler: wrpc_wasi_keyvalue_mem::Handler,
    tx: impl AsyncWrite + Send + Unpin,
    rx: impl AsyncRead + Send + Unpin,
) -> anyhow::Result<()> {
    let mut lis = Listener::new(tx, rx);
    while let Some((Header { instance, name }, tx, rx)) = lis.accept().await? {
        let Ok(fut) = wrpc_wasi_keyvalue::handle(&handler, (), &instance, &name, tx, rx) else {
            bail!("`{instance}#{name}` is not exported");
        };
        fut.await?;
    }
    Ok(())
}

async fn serve_conn(conn: quinn::Incoming) -> anyhow::Result<()> {
    let conn = conn.await.context("failed to accept QUIC connection")?;
    let req = Request::accept(conn)
        .await
        .context("failed to accept WebTransport session request")?;
    let conn = req
        .ok()
        .await
        .context("failed to establish WebTransport connection")?;
    let handler = wrpc_wasi_keyvalue_mem::Handler::default();
    loop {
        let (tx, rx) = conn.accept_bi().await.context("failed to accept stream")?;
        let handler = handler.clone();
        spawn(async move {
            if let Err(err) = serve(handler, tx, rx).await {
                warn!(?err, "failed to serve stream");
            } else {
                info!("stream successfully served");
            }
        });
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let Args { addr } = Args::parse();

    let CertifiedKey { cert, signing_key } = generate_simple_self_signed([
        "localhost".to_string(),
        "::1".to_string(),
        "127.0.0.1".to_string(),
    ])
    .context("failed to generate server certificate")?;

    let mut tls = rustls::ServerConfig::builder_with_protocol_versions(&[&TLS13])
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.into()],
            PrivatePkcs8KeyDer::from(signing_key.serialize_der()).into(),
        )
        .context("failed to create server config")?;
    tls.alpn_protocols = vec![web_transport_quinn::ALPN.as_bytes().to_vec()];
    let tls: QuicServerConfig = tls
        .try_into()
        .context("failed to convert rustls server config to QUIC server config")?;
    let mut transport = quinn::TransportConfig::default();
    transport.keep_alive_interval(Some(Duration::from_secs(3)));
    let mut conf = quinn::ServerConfig::with_crypto(Arc::new(tls));
    conf.transport_config(Arc::new(transport));
    let ep = quinn::Endpoint::server(conf, addr).context("failed to create server endpoint")?;

    let shutdown = signal::ctrl_c();
    let mut shutdown = pin!(shutdown);
    let mut tasks = JoinSet::new();
    loop {
        select! {
            Some(conn) = ep.accept() => {
                debug!("connection accepted");
                tasks.spawn(async move {
                    if let Err(err) = serve_conn(conn).await {
                        warn!(?err, "failed to serve connection");
                    }
                });
            }
            Some(res) = tasks.join_next() => {
                if let Err(err) = res {
                    error!(?err, "failed to join task");
                }
            }
            res = &mut shutdown => return res.context("failed to listen for ^C"),
        }
    }
}
