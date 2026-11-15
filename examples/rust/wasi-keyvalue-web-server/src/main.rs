use core::net::SocketAddr;
use core::pin::pin;
use core::time::Duration;

use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;
use rcgen::{CertifiedKey, generate_simple_self_signed};
use rustls::pki_types::PrivatePkcs8KeyDer;
use rustls::version::TLS13;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::{select, signal};
use tracing::{debug, error, info, warn};
use web_transport_quinn::quinn::crypto::rustls::QuicServerConfig;
use web_transport_quinn::{Request, quinn};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Address to serve `wasi:keyvalue` on
    #[arg(default_value = "[::1]:4433")]
    addr: SocketAddr,
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

    let (tx, mut rx) = mpsc::channel(16);
    let accept = tokio::spawn({
        let mut tasks = JoinSet::<anyhow::Result<()>>::new();
        async move {
            loop {
                select! {
                    Some(conn) = ep.accept() => {
                        let tx = tx.clone();
                        tasks.spawn(async move {
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
                                let (send, recv) = conn.accept_bi().await.context("failed to accept stream")?;
                                let res = tx.send(wrpc_wasi_keyvalue::serve((), send, recv, handler.clone())).await;
                                if res.is_err() {
                                    break;
                                }
                            }
                            Ok(())
                        });
                    }
                    Some(res) = tasks.join_next() => {
                        match res {
                            Ok(Ok(())) => {}
                            Ok(Err(err)) => {
                                warn!(?err, "failed to serve connection");
                            }
                            Err(err) => {
                                error!(?err, "failed to join task");
                            }
                        }
                    }
                    else => return,
                }
            }
        }
    });

    let shutdown = signal::ctrl_c();
    let mut shutdown = pin!(shutdown);
    let mut tasks = JoinSet::new();
    loop {
        select! {
            Some(fut) = rx.recv() => {
                debug!("stream accepted");
                tasks.spawn(async move {
                    if let Err(err) = fut.await {
                        warn!(?err, "failed to serve stream");
                    } else {
                        info!("stream successfully served");
                    }
                });
            }
            Some(res) = tasks.join_next() => {
                if let Err(err) = res {
                    error!(?err, "failed to join task");
                }
            }
            res = &mut shutdown => {
                accept.abort();
                return res.context("failed to listen for ^C")
            }
        }
    }
}
