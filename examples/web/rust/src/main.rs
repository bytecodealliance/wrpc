use core::net::{Ipv6Addr, SocketAddr};
use core::pin::pin;
use core::time::Duration;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context as _;
use axum::Router;
use axum::http::header::CONTENT_TYPE;
use axum::response::Html;
use axum::routing::get;
use bytes::Bytes;
use clap::Parser;
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use rcgen::{CertificateParams, KeyPair};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::version::TLS13;
use rustls::{DigitallySignedStruct, SignatureScheme};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::sync::{Notify, RwLock};
use tokio::task::JoinSet;
use tokio::{select, signal, spawn};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use tracing::{debug, error, info, instrument, trace, warn};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use url::Url;
use uuid::Uuid;
use web_transport_quinn::Request;
use wrpc::{Framed, Header, Listener, ResourceBorrow, ResourceOwn};
use wrpc_wasi_keyvalue::exports::wasi::keyvalue::store;

pub type Result<T, E = store::Error> = core::result::Result<T, E>;

/// This completely disables server TLS certificate verification.
// NOTE: Do not use in production setting!
#[derive(Debug)]
struct Insecure;

impl ServerCertVerifier for Insecure {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ECDSA_NISTP256_SHA256]
    }
}

#[derive(Clone, Debug)]
enum Remote {
    Quic(quinn::Connection),
    Tcp(SocketAddr),
    #[cfg(unix)]
    Unix(std::path::PathBuf),
    Web(Box<web_transport_quinn::Session>),
}

impl Remote {
    async fn connect(
        &self,
    ) -> anyhow::Result<Framed<Box<dyn AsyncRead + Send + Unpin>, Box<dyn AsyncWrite + Send + Unpin>>>
    {
        let (tx, rx): (
            Box<dyn AsyncWrite + Send + Unpin>,
            Box<dyn AsyncRead + Send + Unpin>,
        ) = match self {
            Self::Quic(conn) => {
                let (tx, rx) = conn.open_bi().await?;
                (Box::new(tx), Box::new(rx))
            }
            Self::Tcp(addr) => {
                let stream = TcpStream::connect(addr).await?;
                stream.set_nodelay(true)?;
                let (rx, tx) = stream.into_split();
                (Box::new(tx), Box::new(rx))
            }
            #[cfg(unix)]
            Self::Unix(path) => {
                let stream = tokio::net::UnixStream::connect(path).await?;
                let (rx, tx) = stream.into_split();
                (Box::new(tx), Box::new(rx))
            }
            Self::Web(session) => {
                let (tx, rx) = session.open_bi().await?;
                (Box::new(tx), Box::new(rx))
            }
        };
        Ok(Framed::new(tx, rx))
    }
}

#[derive(Clone, Debug)]
enum Bucket {
    Mem(ResourceOwn<store::Bucket>),
    Redis(ResourceOwn<store::Bucket>),
    Remote(
        ResourceOwn<wrpc_wasi_keyvalue::wasi::keyvalue::store::Bucket>,
        Remote,
    ),
}

#[derive(Clone, Default)]
struct Handler {
    buckets: Arc<RwLock<HashMap<Bytes, Bucket>>>,
    mem: wrpc_wasi_keyvalue_mem::Handler,
    redis: wrpc_wasi_keyvalue_redis::Handler,
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

fn parse_addr(url: &Url, default_port: u16) -> Result<SocketAddr> {
    let auth = url.authority();
    if url.port().is_some() {
        match auth.parse().context("failed to parse IP address") {
            Ok(addr) => Ok(addr),
            Err(err) => Err(store::Error::Other(format!("{err:#}"))),
        }
    } else {
        match auth.parse().context("failed to parse socket address") {
            Ok(ip) => Ok(SocketAddr::new(ip, default_port)),
            Err(err) => Err(store::Error::Other(format!("{err:#}"))),
        }
    }
}

// TODO: Allow TLS configuration via runtime flags
// TODO: Support WebPKI
fn client_tls_config() -> rustls::ClientConfig {
    rustls::ClientConfig::builder_with_protocol_versions(&[&TLS13])
        .dangerous()
        // NOTE: Do not do this in production!
        .with_custom_certificate_verifier(Arc::new(Insecure))
        .with_no_client_auth()
}

impl store::Handler for Handler {
    #[instrument(level = "trace", skip(self), ret(level = "trace"))]
    async fn open(
        &self,
        identifier: String,
    ) -> std::io::Result<Result<ResourceOwn<store::Bucket>>> {
        if identifier.is_empty() {
            let bucket = match self.mem.open(identifier).await? {
                Ok(bucket) => bucket,
                Err(err) => return Ok(Err(err)),
            };
            let id = Uuid::now_v7();
            let id = Bytes::copy_from_slice(id.as_bytes());
            let mut buckets = self.buckets.write().await;
            buckets.insert(id.clone(), Bucket::Mem(bucket));
            return Ok(Ok(ResourceOwn::from(id)));
        }
        let (url, suffix) = identifier.split_once(';').unwrap_or((&identifier, ""));
        let url = match Url::parse(url) {
            Ok(url) => url,
            Err(err) => return Ok(Err(store::Error::Other(err.to_string()))),
        };
        let remote = match url.scheme() {
            "redis" | "rediss" | "redis+sentinel" | "rediss+sentinel" => {
                let bucket = match self.redis.open(identifier).await? {
                    Ok(bucket) => bucket,
                    Err(err) => return Ok(Err(err)),
                };
                let id = Uuid::now_v7();
                let id = Bytes::copy_from_slice(id.as_bytes());
                let mut buckets = self.buckets.write().await;
                buckets.insert(id.clone(), Bucket::Redis(bucket));
                return Ok(Ok(ResourceOwn::from(id)));
            }
            "wrpc+quic" => {
                let addr = match parse_addr(&url, 443) {
                    Ok(addr) => addr,
                    Err(err) => return Ok(Err(err)),
                };
                let mut ep = match quinn::Endpoint::client((Ipv6Addr::UNSPECIFIED, 0).into())
                    .context("failed to create QUIC endpoint")
                {
                    Ok(ep) => ep,
                    Err(err) => return Ok(Err(store::Error::Other(format!("{err:#}")))),
                };
                let san = url.path();
                let mut san = san.strip_prefix('/').unwrap_or(san);
                if san.is_empty() {
                    san = "localhost";
                }
                let conf: QuicClientConfig = client_tls_config()
                    .try_into()
                    .context("failed to convert rustls client config to QUIC client config")
                    .map_err(std::io::Error::other)?;
                ep.set_default_client_config(quinn::ClientConfig::new(Arc::new(conf)));
                let conn = match ep
                    .connect(addr, san)
                    .context("failed to connect using QUIC")
                {
                    Ok(ep) => ep,
                    Err(err) => return Ok(Err(store::Error::Other(format!("{err:#}")))),
                };
                let conn = match conn.await.context("failed to establish QUIC connection") {
                    Ok(ep) => ep,
                    Err(err) => return Ok(Err(store::Error::Other(format!("{err:#}")))),
                };
                Remote::Quic(conn)
            }
            "wrpc+tcp" => {
                let addr = match parse_addr(&url, 7761) {
                    Ok(addr) => addr,
                    Err(err) => return Ok(Err(err)),
                };
                Remote::Tcp(addr)
            }
            #[cfg(unix)]
            "wrpc+unix" => {
                let Ok(path) = url.to_file_path() else {
                    return Ok(Err(store::Error::Other(
                        "failed to get filesystem path from URL".to_string(),
                    )));
                };
                Remote::Unix(path)
            }
            "wrpc+web" => {
                let session_url = match Url::parse(&format!("https://{}", url.authority()))
                    .context("failed to construct WebTransport session URL")
                {
                    Ok(url) => url,
                    Err(err) => return Ok(Err(store::Error::Other(format!("{err:#}")))),
                };
                let ep = match quinn::Endpoint::client((Ipv6Addr::UNSPECIFIED, 0).into())
                    .context("failed to create WebTransport endpoint")
                {
                    Ok(ep) => ep,
                    Err(err) => return Ok(Err(store::Error::Other(format!("{err:#}")))),
                };
                let mut conf = client_tls_config();
                conf.alpn_protocols = vec![web_transport_quinn::ALPN.as_bytes().to_vec()];
                let conf: QuicClientConfig = match conf
                    .try_into()
                    .context("failed to convert rustls client config to QUIC client config")
                {
                    Ok(conf) => conf,
                    Err(err) => return Ok(Err(store::Error::Other(format!("{err:#}")))),
                };
                let ep =
                    web_transport_quinn::Client::new(ep, quinn::ClientConfig::new(Arc::new(conf)));
                let conn = match ep
                    .connect(session_url)
                    .await
                    .context("failed to establish WebTransport connection")
                {
                    Ok(ep) => ep,
                    Err(err) => return Ok(Err(store::Error::Other(format!("{err:#}")))),
                };
                Remote::Web(Box::new(conn))
            }
            scheme => {
                return Ok(Err(store::Error::Other(format!(
                    "unsupported scheme: {scheme}"
                ))));
            }
        };
        let clt = remote
            .connect()
            .await
            .context("failed to connect to remote")
            .map_err(std::io::Error::other)?;
        let bucket = wrpc_wasi_keyvalue::wasi::keyvalue::store::open(clt, suffix)
            .await
            .map_err(std::io::Error::other)?;
        let bucket = match bucket {
            Ok(bucket) => bucket,
            Err(err) => return Ok(Err(err.into())),
        };
        let id = Uuid::now_v7();
        let id = Bytes::copy_from_slice(id.as_bytes());
        let mut buckets = self.buckets.write().await;
        buckets.insert(id.clone(), Bucket::Remote(bucket, remote));
        Ok(Ok(ResourceOwn::from(id)))
    }
}

impl store::HandlerBucket for Handler {
    #[instrument(level = "trace", skip(self), ret(level = "trace"))]
    async fn get(
        &self,
        bucket: ResourceBorrow<store::Bucket>,
        key: String,
    ) -> std::io::Result<Result<Option<Bytes>>> {
        let bucket = self.bucket(bucket).await.map_err(std::io::Error::other)?;
        let res = match bucket {
            Bucket::Mem(bucket) => return self.mem.get(bucket.as_borrow(), key).await,
            Bucket::Redis(bucket) => return self.redis.get(bucket.as_borrow(), key).await,
            Bucket::Remote(bucket, remote) => {
                let clt = remote
                    .connect()
                    .await
                    .context("failed to connect to remote")
                    .map_err(std::io::Error::other)?;
                wrpc_wasi_keyvalue::wasi::keyvalue::store::Bucket::get(
                    clt,
                    &bucket.as_borrow(),
                    &key,
                )
                .await
                .map_err(std::io::Error::other)?
            }
        };
        match res {
            Ok(res) => Ok(Ok(res)),
            Err(err) => Ok(Err(err.into())),
        }
    }

    #[instrument(level = "trace", skip(self), ret(level = "trace"))]
    async fn set(
        &self,
        bucket: ResourceBorrow<store::Bucket>,
        key: String,
        value: Bytes,
    ) -> std::io::Result<Result<()>> {
        let bucket = self.bucket(bucket).await.map_err(std::io::Error::other)?;
        let res = match bucket {
            Bucket::Mem(bucket) => return self.mem.set(bucket.as_borrow(), key, value).await,
            Bucket::Redis(bucket) => {
                return self.redis.set(bucket.as_borrow(), key, value).await;
            }
            Bucket::Remote(bucket, remote) => {
                let clt = remote
                    .connect()
                    .await
                    .context("failed to connect to remote")
                    .map_err(std::io::Error::other)?;
                wrpc_wasi_keyvalue::wasi::keyvalue::store::Bucket::set(
                    clt,
                    &bucket.as_borrow(),
                    &key,
                    &value,
                )
                .await
                .map_err(std::io::Error::other)?
            }
        };
        match res {
            Ok(()) => Ok(Ok(())),
            Err(err) => Ok(Err(err.into())),
        }
    }

    #[instrument(level = "trace", skip(self), ret(level = "trace"))]
    async fn delete(
        &self,
        bucket: ResourceBorrow<store::Bucket>,
        key: String,
    ) -> std::io::Result<Result<()>> {
        let bucket = self.bucket(bucket).await.map_err(std::io::Error::other)?;
        let res = match bucket {
            Bucket::Mem(bucket) => return self.mem.delete(bucket.as_borrow(), key).await,
            Bucket::Redis(bucket) => return self.redis.delete(bucket.as_borrow(), key).await,
            Bucket::Remote(bucket, remote) => {
                let clt = remote
                    .connect()
                    .await
                    .context("failed to connect to remote")
                    .map_err(std::io::Error::other)?;
                wrpc_wasi_keyvalue::wasi::keyvalue::store::Bucket::delete(
                    clt,
                    &bucket.as_borrow(),
                    &key,
                )
                .await
                .map_err(std::io::Error::other)?
            }
        };
        match res {
            Ok(()) => Ok(Ok(())),
            Err(err) => Ok(Err(err.into())),
        }
    }

    #[instrument(level = "trace", skip(self), ret(level = "trace"))]
    async fn exists(
        &self,
        bucket: ResourceBorrow<store::Bucket>,
        key: String,
    ) -> std::io::Result<Result<bool>> {
        let bucket = self.bucket(bucket).await.map_err(std::io::Error::other)?;
        let res = match bucket {
            Bucket::Mem(bucket) => return self.mem.exists(bucket.as_borrow(), key).await,
            Bucket::Redis(bucket) => return self.redis.exists(bucket.as_borrow(), key).await,
            Bucket::Remote(bucket, remote) => {
                let clt = remote
                    .connect()
                    .await
                    .context("failed to connect to remote")
                    .map_err(std::io::Error::other)?;
                wrpc_wasi_keyvalue::wasi::keyvalue::store::Bucket::exists(
                    clt,
                    &bucket.as_borrow(),
                    &key,
                )
                .await
                .map_err(std::io::Error::other)?
            }
        };
        match res {
            Ok(res) => Ok(Ok(res)),
            Err(err) => Ok(Err(err.into())),
        }
    }

    #[instrument(level = "trace", skip(self), ret(level = "trace"))]
    async fn list_keys(
        &self,
        bucket: ResourceBorrow<store::Bucket>,
        cursor: Option<String>,
    ) -> std::io::Result<Result<store::KeyResponse>> {
        let bucket = self.bucket(bucket).await.map_err(std::io::Error::other)?;
        let res = match bucket {
            Bucket::Mem(bucket) => return self.mem.list_keys(bucket.as_borrow(), cursor).await,
            Bucket::Redis(bucket) => {
                return self.redis.list_keys(bucket.as_borrow(), cursor).await;
            }
            Bucket::Remote(bucket, remote) => {
                let clt = remote
                    .connect()
                    .await
                    .context("failed to connect to remote")
                    .map_err(std::io::Error::other)?;
                wrpc_wasi_keyvalue::wasi::keyvalue::store::Bucket::list_keys(
                    clt,
                    &bucket.as_borrow(),
                    cursor.as_deref(),
                )
                .await
                .map_err(std::io::Error::other)?
            }
        };
        match res {
            Ok(res) => Ok(Ok(res.into())),
            Err(err) => Ok(Err(err.into())),
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

async fn serve_conn(handler: Handler, conn: quinn::Incoming) -> anyhow::Result<()> {
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
        let handler = handler.clone();
        spawn(async move {
            let mut lis = Listener::new(tx, rx);
            loop {
                let (Header { instance, name }, srv) = match lis.accept().await {
                    Ok(Some(next)) => next,
                    Ok(None) => return,
                    Err(err) => {
                        warn!(?err, "failed to read invocation header");
                        return;
                    }
                };
                let fut = match store::handle_interface(&handler, &instance, &name, srv) {
                    Ok(fut) => fut,
                    Err(..) => {
                        warn!(instance, func = name, "function is not exported");
                        return;
                    }
                };
                debug!("invocation accepted");
                if let Err(err) = fut.await {
                    warn!(?err, "failed to handle invocation");
                    return;
                }
                info!("invocation successfully handled");
            }
        });
    }
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

    let mut params = CertificateParams::new([
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
    ])
    .context("failed to construct certificate parameters")?;
    params.not_before = time::OffsetDateTime::now_utc();
    params.not_after = params.not_before + time::Duration::days(14);
    let key = KeyPair::generate().context("failed to generate server key")?;
    let cert = params
        .self_signed(&key)
        .context("failed to generate server certificate")?;
    let cert = CertificateDer::from(cert);
    let digest = web_transport_quinn::crypto::sha256(
        &web_transport_quinn::crypto::default_provider(),
        &cert,
    );
    let digest = format!("{:?}", digest.as_ref());

    let mut tls = rustls::ServerConfig::builder_with_protocol_versions(&[&TLS13])
        .with_no_client_auth()
        .with_single_cert(
            vec![cert],
            PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
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
    let ep = quinn::Endpoint::server(conf, (Ipv6Addr::UNSPECIFIED, 0).into())
        .context("failed to create server endpoint")?;
    let ep_addr = ep
        .local_addr()
        .context("failed to query WebTransport socket address")?;
    let port = ep_addr.port();

    let index = get(Html(include_str!("../../ui/index.html")));
    let http = axum::serve(
        lis,
        Router::new()
            .route("/", index.clone())
            .route(
                "/main.js",
                get((
                    [(CONTENT_TYPE, "application/javascript")],
                    include_str!("../../ui/main.js"),
                )),
            )
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
            // Serve the `@bytecodealliance/wrpc` client and the bindings the UI imports.
            .nest_service(
                "/wrpc",
                ServeDir::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../js/src")),
            )
            .nest_service(
                "/bindings",
                ServeDir::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../ui/bindings")),
            )
            .fallback(index)
            .layer(TraceLayer::new_for_http()),
    );

    let handler = Handler::default();

    let http_shutdown = Arc::new(Notify::new());
    let http = http.with_graceful_shutdown({
        let http_shutdown = Arc::clone(&http_shutdown);
        async move { http_shutdown.notified().await }
    });
    let mut http = http.into_future();
    let shutdown = signal::ctrl_c();
    let mut shutdown = pin!(shutdown);
    let mut tasks = JoinSet::new();
    info!("serving HTTP on: http://{addr}");
    loop {
        select! {
            res = &mut http => {
                trace!("HTTP serving stopped");
                return res.context("failed to serve HTTP");
            }
            Some(conn) = ep.accept() => {
                debug!("connection accepted");
                let handler = handler.clone();
                tasks.spawn(async move {
                    if let Err(err) = serve_conn(handler, conn).await {
                        warn!(?err, "failed to serve connection");
                    }
                });
            }
            Some(res) = tasks.join_next() => {
                if let Err(err) = res {
                    error!(?err, "failed to join task");
                }
            }
            res = &mut shutdown => {
                trace!("^C received");
                http_shutdown.notify_waiters();
                ep.close(quinn::VarInt::from_u32(0), b"shutdown");
                ep.wait_idle().await;
                // wait for all WebTransport invocations to complete
                while let Some(res) = tasks.join_next().await {
                    if let Err(err) = res {
                        error!(?err, "failed to WebTransport invocation task");
                    }
                }
                http.await.context("HTTP server failed")?;
                res.context("failed to listen for ^C")?;
                return Ok(())
            }
        }
    }
}
