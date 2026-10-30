use core::net::SocketAddr;
use core::pin::pin;
use core::time::Duration;

use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;
use futures::StreamExt as _;
use futures::stream::select_all;
use rcgen::{CertifiedKey, generate_simple_self_signed};
use rustls::pki_types::PrivatePkcs8KeyDer;
use rustls::version::TLS13;
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

    let srv = Arc::new(wrpc_webtransport::Server::new());
    let accept = tokio::spawn({
        let mut tasks = JoinSet::<anyhow::Result<()>>::new();
        let srv = Arc::clone(&srv);
        async move {
            loop {
                select! {
                    Some(conn) = ep.accept() => {
                        let srv = Arc::clone(&srv);
                        tasks.spawn(async move {
                            let conn = conn.await.context("failed to accept QUIC connection")?;
                            let req = Request::accept(conn)
                                .await
                                .context("failed to accept WebTransport session request")?;
                            let conn = req
                                .ok()
                                .await
                                .context("failed to establish WebTransport connection")?;
                            loop {
                                let (tx, rx) = conn
                                    .accept_bi()
                                    .await
                                    .context("failed to accept wRPC connection")?;
                                srv.accept((), tx.into(), rx.into())
                                    .await
                                    .context("failed to serve wRPC connection")?;
                            }
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

    let invocations =
        wrpc_wasi_keyvalue::serve(srv.as_ref(), wrpc_wasi_keyvalue_mem::Handler::default())
            .await
            .context("failed to serve `wasi:keyvalue`")?;
    // NOTE: This will conflate all invocation streams into a single stream via `futures::stream::SelectAll`,
    // to customize this, iterate over the returned `invocations` and set up custom handling per export
    let mut invocations = select_all(
        invocations
            .into_iter()
            .map(|(instance, name, invocations)| invocations.map(move |res| (instance, name, res))),
    );
    let shutdown = signal::ctrl_c();
    let mut shutdown = pin!(shutdown);
    let mut tasks = JoinSet::new();
    loop {
        select! {
            Some((instance, name, res)) = invocations.next() => {
                match res {
                    Ok(fut) => {
                        debug!(instance, name, "invocation accepted");
                        tasks.spawn(async move {
                            match fut.await { Err(err) => {
                                warn!(?err, "failed to handle invocation");
                            } _ => {
                                info!(instance, name, "invocation successfully handled");
                            }}
                        });
                    }
                    Err(err) => {
                        warn!(?err, instance, name, "failed to accept invocation");
                    }
                }
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
