use core::net::SocketAddr;
use core::pin::pin;

use anyhow::Context as _;
use clap::Parser;
use futures::StreamExt as _;
use rcgen::{CertifiedKey, generate_simple_self_signed};
use rustls::pki_types::PrivatePkcs8KeyDer;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::{select, signal};
use tracing::{debug, error, info, warn};
use web_transport_quinn::{Request, ServerBuilder, quinn};

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
    addr: SocketAddr,
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

    let CertifiedKey { cert, signing_key } = generate_simple_self_signed([
        "localhost".to_string(),
        "::1".to_string(),
        "127.0.0.1".to_string(),
    ])
    .context("failed to generate server certificate")?;

    let ep = ServerBuilder::default()
        .with_addr(addr)
        .with_certificate(
            vec![cert.into()],
            PrivatePkcs8KeyDer::from(signing_key.serialize_der()).into(),
        )
        .context("failed to create server endpoint")?;

    let (tx, mut rx) = mpsc::channel(16);

    let accept = tokio::spawn(async move {
        let mut tasks = JoinSet::<anyhow::Result<()>>::new();
        while let Some(conn) = quinn::Endpoint::accept(&ep).await {
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
                let conn = wrpc_webtransport::Session::from(conn);
                let invocations = bindings::serve(conn.incoming(), Handler);
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
