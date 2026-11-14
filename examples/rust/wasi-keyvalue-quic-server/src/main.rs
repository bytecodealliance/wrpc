use core::net::SocketAddr;
use core::pin::pin;

use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;
use futures::StreamExt as _;
use quinn::crypto::rustls::QuicServerConfig;
use quinn::{Endpoint, ServerConfig};
use rcgen::{CertifiedKey, generate_simple_self_signed};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use rustls::version::TLS13;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::{select, signal};
use tracing::{debug, error, info, warn};

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
    let cert = CertificateDer::from(cert);

    let conf = rustls::ServerConfig::builder_with_protocol_versions(&[&TLS13])
        .with_no_client_auth() // TODO: verify client cert
        .with_single_cert(
            vec![cert],
            PrivatePkcs8KeyDer::from(signing_key.serialize_der()).into(),
        )
        .context("failed to create server config")?;

    let conf: QuicServerConfig = conf
        .try_into()
        .context("failed to convert rustls client config to QUIC server config")?;

    let ep = Endpoint::server(ServerConfig::with_crypto(Arc::new(conf)), addr)
        .context("failed to create server endpoint")?;

    let (tx, mut rx) = mpsc::channel(16);
    let accept = tokio::spawn({
        let mut tasks = JoinSet::<anyhow::Result<()>>::new();
        async move {
            loop {
                select! {
                    Some(conn) = ep.accept() => {
                        let tx = tx.clone();
                        tasks.spawn(async move {
                            let conn = conn
                                .accept()
                                .context("failed to accept QUIC connection")?;
                            let conn = conn.await.context("failed to establish QUIC connection")?;
                            let conn = wrpc_quic::Connection::from(conn);
                            let invocations = wrpc_wasi_keyvalue::serve(
                                conn.incoming(),
                                wrpc_wasi_keyvalue_mem::Handler::default(),
                            );
                            let mut invocations = pin!(invocations);
                            while let Some(res) = invocations.next().await {
                                let fut = res.context("failed to accept wRPC invocation")?;
                                if tx.send(fut).await.is_err() {
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
                    else => {
                        return;
                    }
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
                debug!("invocation accepted");
                tasks.spawn(async move {
                    match fut.await { Err(err) => {
                        warn!(?err, "failed to handle invocation");
                    } _ => {
                        info!("invocation successfully handled");
                    }}
                });
            }
            Some(res) = tasks.join_next() => {
                if let Err(err) = res {
                    error!(?err, "failed to join task");
                }
            }
            res = &mut shutdown => {
                accept.abort();
                // wait for all invocations to complete
                while let Some(res) = tasks.join_next().await {
                    if let Err(err) = res {
                        error!(?err, "failed to join task");
                    }
                }
                return res.context("failed to listen for ^C")
            }
        }
    }
}
