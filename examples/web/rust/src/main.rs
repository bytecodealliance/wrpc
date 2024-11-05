use core::net::SocketAddr;
use core::pin::pin;
use core::time::Duration;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context as _;
use axum::http::header::CONTENT_TYPE;
use axum::response::Html;
use axum::routing::get;
use axum::Router;
use bytes::Bytes;
use clap::Parser;
use futures::stream::select_all;
use futures::StreamExt as _;
use tokio::sync::RwLock;
use tokio::task::JoinSet;
use tokio::{select, signal};
use tower_http::trace::TraceLayer;
use tracing::{debug, error, info, instrument, trace, warn};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use url::Url;
use uuid::Uuid;
use wrpc_transport::{ResourceBorrow, ResourceOwn};
use wrpc_wasi_keyvalue::exports::wasi::keyvalue::store;
use wtransport::tls::Sha256DigestFmt;
use wtransport::{Endpoint, Identity, ServerConfig};

pub type Result<T, E = store::Error> = core::result::Result<T, E>;

#[derive(Clone, Debug, Default)]
struct Handler {
    buckets: Arc<RwLock<HashMap<Bytes, Bucket>>>,
    mem: wrpc_wasi_keyvalue_mem::Handler,
}

impl Handler {
    async fn bucket(&self, bucket: impl AsRef<[u8]>) -> Result<Bucket> {
        trace!("looking up bucket");
        let store = self.buckets.read().await;
        store
            .get(bucket.as_ref())
            .ok_or(store::Error::NoSuchStore)
            .cloned()
    }
}

#[derive(Clone, Debug)]
enum Bucket {
    Mem(ResourceOwn<store::Bucket>),
}

impl<C: Send + Sync> store::Handler<C> for Handler {
    // NOTE: Resource handle returned is just the `identifier` itself
    #[instrument(level = "trace", skip(cx), ret(level = "trace"))]
    async fn open(
        &self,
        cx: C,
        identifier: String,
    ) -> anyhow::Result<Result<ResourceOwn<store::Bucket>>> {
        if identifier.is_empty() {
            let bucket = match self.mem.open(cx, identifier).await? {
                Ok(bucket) => bucket,
                Err(err) => return Ok(Err(err)),
            };
            let id = Uuid::now_v7();
            let id = Bytes::copy_from_slice(id.as_bytes());
            let mut buckets = self.buckets.write().await;
            buckets.insert(id.clone(), Bucket::Mem(bucket));
            return Ok(Ok(ResourceOwn::from(id)));
        }
        let url = match Url::parse(&identifier) {
            Ok(url) => url,
            Err(err) => return Ok(Err(store::Error::Other(err.to_string()))),
        };
        match url.scheme() {
            "wrpc+nats" => {
                eprintln!("TODO: NATS");
            }
            "wrpc+quic" => {
                eprintln!("TODO: QUIC");
            }
            "wrpc+tcp" => {
                eprintln!("TODO: TCP");
            }
            "wrpc+uds" => {
                eprintln!("TODO: UDS");
            }
            "wrpc+web" => {
                eprintln!("WebTransport");
            }
            scheme => {
                return Ok(Err(store::Error::Other(format!(
                    "unsupported scheme: {scheme}"
                ))))
            }
        }
        return Ok(Err(store::Error::Other("unsupported".into())));
    }
}

impl<C: Send + Sync> store::HandlerBucket<C> for Handler {
    #[instrument(level = "trace", skip(cx), ret(level = "trace"))]
    async fn get(
        &self,
        cx: C,
        bucket: ResourceBorrow<store::Bucket>,
        key: String,
    ) -> anyhow::Result<Result<Option<Bytes>>> {
        match self.bucket(bucket).await? {
            Bucket::Mem(bucket) => self.mem.get(cx, bucket.as_borrow(), key).await,
        }
    }

    #[instrument(level = "trace", skip(cx), ret(level = "trace"))]
    async fn set(
        &self,
        cx: C,
        bucket: ResourceBorrow<store::Bucket>,
        key: String,
        value: Bytes,
    ) -> anyhow::Result<Result<()>> {
        match self.bucket(bucket).await? {
            Bucket::Mem(bucket) => self.mem.set(cx, bucket.as_borrow(), key, value).await,
        }
    }

    #[instrument(level = "trace", skip(cx), ret(level = "trace"))]
    async fn delete(
        &self,
        cx: C,
        bucket: ResourceBorrow<store::Bucket>,
        key: String,
    ) -> anyhow::Result<Result<()>> {
        match self.bucket(bucket).await? {
            Bucket::Mem(bucket) => self.mem.delete(cx, bucket.as_borrow(), key).await,
        }
    }

    #[instrument(level = "trace", skip(cx), ret(level = "trace"))]
    async fn exists(
        &self,
        cx: C,
        bucket: ResourceBorrow<store::Bucket>,
        key: String,
    ) -> anyhow::Result<Result<bool>> {
        match self.bucket(bucket).await? {
            Bucket::Mem(bucket) => self.mem.exists(cx, bucket.as_borrow(), key).await,
        }
    }

    #[instrument(level = "trace", skip(cx), ret(level = "trace"))]
    async fn list_keys(
        &self,
        cx: C,
        bucket: ResourceBorrow<store::Bucket>,
        cursor: Option<String>,
    ) -> anyhow::Result<Result<store::KeyResponse>> {
        match self.bucket(bucket).await? {
            Bucket::Mem(bucket) => self.mem.list_keys(cx, bucket.as_borrow(), cursor).await,
        }
    }
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Address to serve web interface on
    #[arg(default_value = "[::1]:8080")]
    addr: SocketAddr,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().compact().without_time())
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
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
        Handler::default(),
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
