use core::net::SocketAddr;
use core::pin::{Pin, pin};

use anyhow::{Context as _, bail};
use bytes::Bytes;
use clap::Parser;
use futures::Stream;
use rcgen::{CertifiedKey, generate_simple_self_signed};
use rustls::pki_types::PrivatePkcs8KeyDer;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::task::JoinSet;
use tokio::{select, signal, spawn};
use tracing::{debug, error, info, warn};
use web_transport_quinn::{Request, ServerBuilder, quinn};
use wit_bindgen_wrpc::wrpc_transport::{Header, Listener};

mod bindings {
    wit_bindgen_wrpc::generate!({
        with: {
            "wrpc-examples:streams/handler": generate,
        }
    });
}

use bindings::exports::wrpc_examples::streams::handler::Req;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Address to serve `wrpc-examples:streams/handler.echo` on
    #[arg(default_value = "[::1]:4434")]
    addr: SocketAddr,
}

#[derive(Clone, Copy)]
struct Server;

impl bindings::exports::wrpc_examples::streams::handler::Handler for Server {
    async fn echo(
        &self,
        Req { numbers, bytes }: Req,
    ) -> anyhow::Result<(
        Pin<Box<dyn Stream<Item = Vec<u64>> + Send>>,
        Pin<Box<dyn Stream<Item = Bytes> + Send>>,
    )> {
        Ok((numbers, bytes))
    }
}

async fn serve(
    tx: impl AsyncWrite + Send + Unpin,
    rx: impl AsyncRead + Send + Unpin,
) -> anyhow::Result<()> {
    let mut lis = Listener::new(tx, rx);
    while let Some((Header { instance, name }, srv)) = lis.accept().await? {
        let Ok(fut) = bindings::handle(&Server, &instance, &name, srv) else {
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
    loop {
        let (tx, rx) = conn.accept_bi().await.context("failed to accept stream")?;
        spawn(async move {
            if let Err(err) = serve(tx, rx).await {
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

    let ep = ServerBuilder::default()
        .with_addr(addr)
        .with_certificate(
            vec![cert.into()],
            PrivatePkcs8KeyDer::from(signing_key.serialize_der()).into(),
        )
        .context("failed to create server endpoint")?;

    let shutdown = signal::ctrl_c();
    let mut shutdown = pin!(shutdown);
    let mut tasks = JoinSet::new();
    loop {
        select! {
            Some(conn) = quinn::Endpoint::accept(&ep) => {
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
