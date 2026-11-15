use core::net::SocketAddr;
use core::pin::pin;

use anyhow::Context as _;
use clap::Parser;
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
                loop {
                    let (send, recv) = conn.accept_bi().await.context("failed to accept stream")?;
                    let res = tx.send(bindings::serve((), send, recv, Handler)).await;
                    if res.is_err() {
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
