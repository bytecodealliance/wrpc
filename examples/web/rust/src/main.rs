use core::net::SocketAddr;
use core::pin::pin;
use core::time::Duration;

use std::sync::Arc;

use anyhow::Context as _;
use axum::http::header::CONTENT_TYPE;
use axum::response::Html;
use axum::routing::get;
use axum::Router;
use clap::Parser;
use futures::stream::select_all;
use futures::StreamExt as _;
use tokio::task::JoinSet;
use tokio::{select, signal};
use tower_http::trace::TraceLayer;
use tracing::{debug, error, info, trace, warn};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use wtransport::tls::Sha256DigestFmt;
use wtransport::{Endpoint, Identity, ServerConfig};

//#[derive(Clone, Copy)]
//struct Handler;
//
//impl bindings::exports::wrpc_examples::hello::handler::Handler<()> for Handler {
//    async fn hello(&self, (): ()) -> anyhow::Result<String> {
//        Ok("hello from Rust".to_string())
//    }
//}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Address to serve web interface on
    #[arg(default_value = "[::1]:8080")]
    addr: SocketAddr,
}

#[must_use]
pub fn env_filter() -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().compact().without_time())
        .with(env_filter())
        .init();

    let Args { addr } = Args::parse();

    let lis = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("failed to bind TCP listener on `{addr}`"))?;

    let id = Identity::self_signed(["localhost", "127.0.0.1", "::1"])
        .context("failed to generate server certificate")?;
    let digest = id.certificate_chain().as_slice()[0]
        .hash()
        .fmt(Sha256DigestFmt::BytesArray);

    let ep = Endpoint::server(
        ServerConfig::builder()
            .with_bind_default(0)
            .with_identity(id)
            .keep_alive_interval(Some(Duration::from_secs(3)))
            .build(),
    )
    .context("failed to create server endpoint")?;
    let ep_addr = ep
        .local_addr()
        .context("failed to query WebTransport socket address")?;
    let port = ep_addr.port();

    let index = get(Html(include_str!("../index.html")));
    let http = axum::serve(
        lis,
        Router::new()
            .route("/", index.clone())
            .route(
                "/consts.js",
                get((
                    [(CONTENT_TYPE, "application/javascript")],
                    format!(
                        r#"
export const CERT_DIGEST = new Uint8Array({digest});
export const PORT = "{port}"
"#
                    ),
                )),
            )
            .fallback(index)
            .layer(TraceLayer::new_for_http()),
    );

    let srv = Arc::new(wrpc_transport_web::Server::new());
    let webt = tokio::spawn({
        let mut tasks = JoinSet::<anyhow::Result<()>>::new();
        let srv = Arc::clone(&srv);
        async move {
            loop {
                select! {
                    conn = ep.accept() => {
                        let srv = Arc::clone(&srv);
                        tasks.spawn(async move {
                            let req = conn
                                .await
                                .context("failed to accept WebTransport connection")?;
                            let conn = req
                                .accept()
                                .await
                                .context("failed to establish WebTransport connection")?;
                            let wrpc = wrpc_transport_web::Client::from(conn);
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
                }
            }
        }
    });

    let invocations = wrpc_wasi_keyvalue::exports::wasi::keyvalue::store::serve_interface(
        srv.as_ref(),
        wrpc_wasi_keyvalue_mem::Handler::default(),
    )
    .await
    .context("failed to serve `wasi:keyvalue`")?;
    let mut invocations = select_all(
        invocations
            .into_iter()
            .map(|(instance, name, invocations)| invocations.map(move |res| (instance, name, res))),
    );
    let wrpc = tokio::spawn(async move {
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
                else => {
                    return
                }
            }
        }
    });

    let shutdown = signal::ctrl_c();
    let mut shutdown = pin!(shutdown);
    select! {
        res = http => {
            trace!("HTTP serving task stopped");
            res.context("failed to serve HTTP")?;
        }
        res = webt => {
            trace!("WebTransport serving task stopped");
            res.context("failed to serve WebTransport")?;
        }
        res = wrpc => {
            trace!("wRPC serving task stopped");
            res.context("failed to serve wRPC invocations")?;
        }
        res = &mut shutdown => {
            trace!("^C received");
            res.context("failed to listen for ^C")?;
        }
    }
    Ok(())
}
