use core::net::SocketAddr;
use core::pin::pin;

use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;
use futures::stream::select_all;
use futures::StreamExt as _;
use quinn::crypto::rustls::QuicServerConfig;
use quinn::{Endpoint, ServerConfig};
use rcgen::{generate_simple_self_signed, CertifiedKey};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use rustls::version::TLS13;
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

    let CertifiedKey { cert, key_pair } = generate_simple_self_signed([
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
            PrivatePkcs8KeyDer::from(key_pair.serialize_der()).into(),
        )
        .context("failed to create server config")?;

    let conf: QuicServerConfig = conf
        .try_into()
        .context("failed to convert rustls client config to QUIC server config")?;

    let ep = Endpoint::server(ServerConfig::with_crypto(Arc::new(conf)), addr)
        .context("failed to create server endpoint")?;

    let srv = Arc::new(wrpc_transport_quic::Server::new());
    let accept = tokio::spawn({
        let mut tasks = JoinSet::<anyhow::Result<()>>::new();
        let srv = Arc::clone(&srv);
        async move {
            loop {
                select! {
                    Some(conn) = ep.accept() => {
                        let srv = Arc::clone(&srv);
                        tasks.spawn(async move {
                            let conn = conn
                                .accept()
                                .context("failed to accept QUIC connection")?;
                            let conn = conn.await.context("failed to establish QUIC connection")?;
                            let wrpc = wrpc_transport_quic::Client::from(conn);
                            loop {
                                srv.accept(&wrpc)
                                    .await
                                    .context("failed to accept wRPC connection")?;
                            }
                        });
                    }
                    Some(res) = tasks.join_next() => {
                        match res {
                            Ok(Ok(())) => {}
                            Ok(Err(err)) => {
                                warn!(?err, "failed to serve connection")
                            }
                            Err(err) => {
                                error!(?err, "failed to join task")
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
                            if let Err(err) = fut.await {
                                warn!(?err, "failed to handle invocation");
                            } else {
                                info!(instance, name, "invocation successfully handled");
                            }
                        });
                    }
                    Err(err) => {
                        warn!(?err, instance, name, "failed to accept invocation");
                    }
                }
            }
            Some(res) = tasks.join_next() => {
                if let Err(err) = res {
                    error!(?err, "failed to join task")
                }
            }
            res = &mut shutdown => {
                accept.abort();
                // wait for all invocations to complete
                while let Some(res) = tasks.join_next().await {
                    if let Err(err) = res {
                        error!(?err, "failed to join task")
                    }
                }
                return res.context("failed to listen for ^C")
            }
        }
    }
}
