use core::pin::pin;

use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;
use futures::stream::select_all;
use futures::StreamExt as _;
use quinn::{Endpoint, EndpointConfig, ServerConfig, TokioRuntime};
use rcgen::{generate_simple_self_signed, CertifiedKey};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use tokio::task::JoinSet;
use tokio::{select, signal};
use tracing::{debug, error, info, warn};

mod bindings {
    wit_bindgen_wrpc::generate!({
        with: {
            "wrpc-examples:hello/handler": generate,
        }
    });
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Address to serve `wrpc-examples:hello/handler.hello` on
    #[arg(default_value = "[::1]:4433")]
    addr: String,
}

#[derive(Clone, Copy)]
struct Handler;

impl bindings::exports::wrpc_examples::hello::handler::Handler<()> for Handler {
    async fn hello(&self, (): ()) -> anyhow::Result<String> {
        Ok("hello from Rust".to_string())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let Args { addr } = Args::parse();

    let sock = std::net::UdpSocket::bind(addr).context("failed to open a UDP socket")?;

    let CertifiedKey { cert, key_pair } = generate_simple_self_signed(["localhost".to_string()])
        .context("failed to generate server certificate")?;
    let cert = CertificateDer::from(cert);

    let conf = ServerConfig::with_single_cert(
        vec![cert],
        PrivatePkcs8KeyDer::from(key_pair.serialize_der()).into(),
    )
    .context("failed to create server config")?;

    let ep = Endpoint::new(
        EndpointConfig::default(),
        Some(conf),
        sock,
        Arc::new(TokioRuntime),
    )
    .context("failed to create server endpoint")?;

    let srv = Arc::new(wrpc_transport_web::Server::new());
    let invocations = bindings::serve(srv.as_ref(), Handler)
        .await
        .context("failed to serve `wrpc-examples.hello/handler.hello`")?;

    let accept = tokio::spawn(async move {
        let mut tasks = JoinSet::<anyhow::Result<()>>::new();
        while let Some(conn) = ep.accept().await {
            let srv = Arc::clone(&srv);
            tasks.spawn(async move {
                let conn = conn.await.context("failed to accept QUIC connection")?;
                let req = web_transport_quinn::accept(conn)
                    .await
                    .context("failed to accept WebTransport connection")?;
                let session = req
                    .ok()
                    .await
                    .context("failed to establish WebTransport connection")?;
                let wrpc = wrpc_transport_web::Client::from(session);
                loop {
                    srv.accept(&wrpc)
                        .await
                        .context("failed to accept wRPC connection")?;
                }
            });
        }
    });

    // NOTE: This will conflate all invocation streams into a single stream via `futures::stream::SelectAll`,
    // to customize this, iterate over the returned `invocations` and set up custom handling per export
    let mut invocations = select_all(
        invocations
            .into_iter()
            .map(|(instance, name, invocations)| invocations.map(move |res| (instance, name, res))),
    );
    let mut tasks = JoinSet::new();
    let shutdown = signal::ctrl_c();
    let mut shutdown = pin!(shutdown);
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
