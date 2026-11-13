use core::future::poll_fn;
use core::net::Ipv6Addr;
use core::pin::pin;
use core::task::Poll;
use core::time::Duration;

use std::process::ExitStatus;

use anyhow::{Context, ensure};
use bytes::Bytes;
use futures::stream::FuturesUnordered;
use futures::{Stream, StreamExt as _, TryStreamExt as _};
use rcgen::{CertifiedKey, generate_simple_self_signed};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use rustls::version::TLS13;
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};
use tokio::process::Command;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio::{select, spawn, try_join};
use tokio_util::codec::BytesCodec;
use tracing::info;
use wrpc_transport::frame::{Incoming, Outgoing};
use wrpc_transport::{Connect, Header, HeaderReadError, Index as _, accept, invoke, serve};

/// Maximum number of invocations [`serve_until`] serves concurrently
pub const MAX_INVOCATIONS: usize = 64;

async fn write(tx: &mut Outgoing, buf: &'static [u8]) -> std::io::Result<()> {
    tx.encode(&mut BytesCodec::default(), Bytes::from_static(buf))
        .await
}

async fn read_to_end(rx: &mut Incoming) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::default();
    while let Some(chunk) = rx.decode(&mut BytesCodec::default()).await? {
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// Polls `fut` to completion while accepting invocations from `incoming`, reading the header
/// of each and polling the future `f` returns for it, then completes the served invocations.
/// At most [`MAX_INVOCATIONS`] are served concurrently. Returns the first error.
pub async fn serve_until<C, I, O, E, T, Fut>(
    incoming: impl Stream<Item = Result<(C, O, I), E>>,
    fut: impl Future<Output = anyhow::Result<T>>,
    mut f: impl FnMut(C, Header, O, I) -> Fut,
) -> anyhow::Result<T>
where
    I: AsyncRead + Unpin,
    E: core::error::Error + Send + Sync + 'static,
    Fut: Future<Output = anyhow::Result<()>>,
{
    let mut fut = pin!(fut);
    let mut incoming = pin!(incoming);
    let mut reading = FuturesUnordered::default();
    let mut served = FuturesUnordered::default();
    let res = poll_fn(|ctx| {
        while reading.len().saturating_add(served.len()) < MAX_INVOCATIONS
            && let Poll::Ready(Some(res)) = incoming.poll_next_unpin(ctx)
        {
            let (cx, tx, mut rx) = res.context("failed to accept invocation")?;
            reading.push(async move {
                let header = accept(&mut rx).await?;
                Ok::<_, HeaderReadError>((cx, header, tx, rx))
            });
        }
        while let Poll::Ready(Some(res)) = reading.poll_next_unpin(ctx) {
            let (cx, header, tx, rx) = res.context("failed to read invocation header")?;
            served.push(f(cx, header, tx, rx));
        }
        while let Poll::Ready(Some(res)) = served.poll_next_unpin(ctx) {
            res?;
            ctx.waker().wake_by_ref();
        }
        fut.as_mut().poll(ctx)
    })
    .await?;
    served.try_collect::<()>().await?;
    Ok(res)
}

/// Accepts an invocation of function `func` on instance `instance` from `incoming`, returning
/// its context and framed streams, which multiplex the async `paths`
pub async fn accept_framed<C, I, O, E, P>(
    incoming: &mut (impl Stream<Item = Result<(C, O, I), E>> + Unpin),
    instance: &str,
    func: &str,
    paths: impl IntoIterator<Item = P>,
) -> anyhow::Result<(C, Outgoing, Incoming)>
where
    I: AsyncRead + Send + Unpin + 'static,
    O: AsyncWrite + Send + Unpin + 'static,
    E: core::error::Error + Send + Sync + 'static,
    P: AsRef<[Option<usize>]>,
{
    let res = incoming.next().await.context("invocation stream ended")?;
    let (cx, tx, mut rx) = res.context("failed to accept invocation")?;
    let header = accept(&mut rx)
        .await
        .context("failed to read invocation header")?;
    ensure!(
        header.instance == instance && header.name == func,
        "unexpected invocation of `{}.{}`",
        header.instance,
        header.name,
    );
    let (tx, rx) = serve(tx, rx, paths);
    Ok((cx, tx, rx))
}

#[cfg(feature = "net")]
pub async fn serve_tcp<T>(
    f: impl AsyncFnOnce(
        wrpc_transport::tcp::Client<core::net::SocketAddr>,
        &TcpListener,
    ) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let lis = tcp_bind().await?;
    let addr = lis
        .local_addr()
        .context("failed to query listener local address")?;
    f(wrpc_transport::tcp::Client::from(addr), &lis).await
}

#[cfg(all(unix, feature = "net"))]
pub async fn serve_unix<T>(
    f: impl AsyncFnOnce(
        wrpc_transport::unix::Client<std::path::PathBuf>,
        &tokio::net::UnixListener,
    ) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let dir = tempfile::tempdir().context("failed to create temporary directory")?;
    let path = dir.path().join("sock");
    let lis = tokio::net::UnixListener::bind(&path).context("failed to bind Unix socket")?;
    f(wrpc_transport::unix::Client::from(path), &lis).await
}

pub async fn tcp_bind() -> anyhow::Result<TcpListener> {
    TcpListener::bind((Ipv6Addr::LOCALHOST, 0))
        .await
        .context("failed to start TCP listener")
}

pub async fn tcp_connect(addr: impl ToSocketAddrs) -> anyhow::Result<TcpStream> {
    let stream = TcpStream::connect(addr)
        .await
        .context("failed to connect to server")?;
    stream
        .set_nodelay(true)
        .context("failed to set TCP_NODELAY")?;
    Ok(stream)
}

pub async fn free_port() -> anyhow::Result<u16> {
    let sock = tcp_bind().await?;
    sock.local_addr()
        .context("failed to query listener local address")
        .map(|v| v.port())
}

/// A common single-invocation test used by different transport implementations.
///
/// The invocation is accepted from `srv`.
pub async fn assert_single_invocation<T, C, I, O, E>(
    cx: T::Context,
    clt: &T,
    srv: impl Stream<Item = Result<(C, O, I), E>>,
) -> anyhow::Result<C>
where
    T: Connect,
    I: AsyncRead + Send + Unpin + 'static,
    O: AsyncWrite + Send + Unpin + 'static,
    E: core::error::Error + Send + Sync + 'static,
{
    let mut srv = pin!(srv);
    let ((), cx) = try_join!(
        async {
            let (tx, rx) = clt.connect(cx).await.context("failed to connect")?;
            let (mut outgoing, mut incoming) = invoke(tx, rx, "foo", "bar", [&[Some(0), Some(42)]])
                .context("failed to invoke `foo.bar`")?;
            let mut nested_tx = outgoing.index(&[42, 0]).context("failed to index `42.0`")?;
            let mut nested_rx = incoming.index(&[0, 42]).context("failed to index `0.42`")?;
            write(&mut outgoing, b"testbar")
                .await
                .context("failed to write `foo.bar` parameters")?;
            outgoing
                .close()
                .await
                .context("failed to close `foo.bar` parameters")?;
            try_join!(
                async {
                    info!("reading `foo`");
                    let buf = read_to_end(&mut incoming)
                        .await
                        .context("failed to read `foo`")?;
                    assert_eq!(buf, b"foo");
                    info!("read `foo`");
                    anyhow::Ok(())
                },
                async {
                    info!("writing `client->server`");
                    write(&mut nested_tx, b"client->server")
                        .await
                        .context("failed to write `client->server`")?;
                    nested_tx
                        .close()
                        .await
                        .context("failed to close `client->server`")?;
                    info!("wrote `client->server`");
                    anyhow::Ok(())
                },
                async {
                    info!("reading `server->client`");
                    let buf = read_to_end(&mut nested_rx)
                        .await
                        .context("failed to read `server->client`")?;
                    assert_eq!(buf, b"server->client");
                    info!("read `server->client`");
                    anyhow::Ok(())
                },
            )?;
            incoming
                .closed()
                .await
                .context("`foo.bar` invocation failed")?;
            anyhow::Ok(())
        },
        async {
            let (cx, mut outgoing, mut incoming) =
                accept_framed(&mut srv, "foo", "bar", [[Some(42), Some(0)]]).await?;
            let mut nested_tx = outgoing.index(&[0, 42]).context("failed to index `0.42`")?;
            let mut nested_rx = incoming.index(&[42, 0]).context("failed to index `42.0`")?;
            try_join!(
                async {
                    info!("reading `testbar`");
                    let buf = read_to_end(&mut incoming)
                        .await
                        .context("failed to read `testbar`")?;
                    assert_eq!(buf, b"testbar");
                    info!("read `testbar`");
                    anyhow::Ok(())
                },
                async {
                    info!("writing `foo`");
                    write(&mut outgoing, b"foo")
                        .await
                        .context("failed to write `foo`")?;
                    outgoing.close().await.context("failed to close `foo`")?;
                    info!("wrote `foo`");
                    anyhow::Ok(())
                },
                async {
                    info!("writing `server->client`");
                    write(&mut nested_tx, b"server->client")
                        .await
                        .context("failed to write `server->client`")?;
                    nested_tx
                        .close()
                        .await
                        .context("failed to close `server->client`")?;
                    info!("wrote `server->client`");
                    anyhow::Ok(())
                },
                async {
                    info!("reading `client->server`");
                    let buf = read_to_end(&mut nested_rx)
                        .await
                        .context("failed to read `client->server`")?;
                    assert_eq!(buf, b"client->server");
                    info!("read `client->server`");
                    anyhow::Ok(())
                },
            )?;
            anyhow::Ok(cx)
        },
    )?;
    Ok(cx)
}

/// A common test used by different transport implementations, which checks that invalid data
/// fails ingress of a server and a client, and that dropping the writer of the failed side ends
/// the invocation for the peer.
///
/// The invocations are accepted from `srv`.
pub async fn assert_invalid_data_resets<T, C, I, O, E>(
    cx: T::Context,
    clt: &T,
    srv: impl Stream<Item = Result<(C, O, I), E>>,
) -> anyhow::Result<()>
where
    T: Connect,
    T::Context: Clone,
    I: AsyncRead + Send + Unpin + 'static,
    O: AsyncWrite + Send + Unpin + 'static,
    E: core::error::Error + Send + Sync + 'static,
{
    let mut srv = pin!(srv);
    let (tx, rx) = clt.connect(cx.clone()).await.context("failed to connect")?;
    let (outgoing, mut incoming) =
        invoke(tx, rx, "foo", "baz", [[None; 0]; 0]).context("failed to invoke `foo.baz`")?;
    let mut invalid = outgoing.index(&[1]).context("failed to index `1`")?;
    write(&mut invalid, b"x")
        .await
        .context("failed to write invalid data")?;
    let (.., outgoing, srv_incoming) = accept_framed(&mut srv, "foo", "baz", [[Some(0)]]).await?;
    let finished = srv_incoming.closed();
    finished
        .await
        .expect_err("invalid data must fail server ingress");
    drop(outgoing);
    let res = timeout(Duration::from_secs(5), read_to_end(&mut incoming))
        .await
        .context("invalid data did not reset the invocation")?;
    res.expect_err("invalid data must reset the invocation");

    let (tx, rx) = clt.connect(cx).await.context("failed to connect")?;
    let (mut clt_outgoing, incoming) =
        invoke(tx, rx, "foo", "baz", [[None; 0]; 0]).context("failed to invoke `foo.baz`")?;
    clt_outgoing
        .flush()
        .await
        .context("failed to send invocation header")?;
    let (.., outgoing, srv_incoming) = accept_framed(&mut srv, "foo", "baz", [[Some(0)]]).await?;
    let mut nested = srv_incoming.index(&[0]).context("failed to index `0`")?;
    let mut invalid = outgoing.index(&[1]).context("failed to index `1`")?;
    write(&mut invalid, b"x")
        .await
        .context("failed to write invalid data")?;
    incoming
        .closed()
        .await
        .expect_err("invalid data must fail client ingress");
    drop(clt_outgoing);
    let res = timeout(Duration::from_secs(5), read_to_end(&mut nested))
        .await
        .context("invalid data did not reset the invocation")?;
    res.expect_err("invalid data must reset the invocation");
    Ok(())
}

pub fn spawn_server(
    cmd: &mut Command,
) -> anyhow::Result<(JoinHandle<anyhow::Result<ExitStatus>>, oneshot::Sender<()>)> {
    let mut child = cmd
        .kill_on_drop(true)
        .spawn()
        .context("failed to spawn child")?;
    let (stop_tx, stop_rx) = oneshot::channel();
    let child = spawn(async move {
        select!(
            res = stop_rx => {
                res.context("failed to wait for shutdown")?;
                child.kill().await.context("failed to kill child")?;
                child.wait().await
            }
            status = child.wait() => {
                status
            }
        )
        .context("failed to wait for child")
    });
    Ok((child, stop_tx))
}

pub fn cert_pair() -> anyhow::Result<(rustls::ServerConfig, rustls::ClientConfig)> {
    let CertifiedKey {
        cert: srv_crt,
        signing_key: srv_key,
    } = generate_simple_self_signed([
        "127.0.0.1".to_string(),
        "::1".to_string(),
        "localhost".to_string(),
    ])
    .context("failed to generate server certificate")?;
    let CertifiedKey {
        cert: clt_crt,
        signing_key: clt_key,
    } = generate_simple_self_signed(["client.wrpc".to_string()])
        .context("failed to generate client certificate")?;
    let srv_crt = CertificateDer::from(srv_crt);

    let mut ca = RootCertStore::empty();
    ca.add(srv_crt.clone())?;
    let clt_cnf = ClientConfig::builder_with_protocol_versions(&[&TLS13])
        .with_root_certificates(ca)
        .with_client_auth_cert(
            vec![clt_crt.into()],
            PrivatePkcs8KeyDer::from(clt_key.serialize_der()).into(),
        )
        .context("failed to create client config")?;
    let srv_cnf = ServerConfig::builder_with_protocol_versions(&[&TLS13])
        .with_no_client_auth() // TODO: verify client cert
        .with_single_cert(
            vec![srv_crt],
            PrivatePkcs8KeyDer::from(srv_key.serialize_der()).into(),
        )
        .context("failed to create server config")?;
    Ok((srv_cnf, clt_cnf))
}

#[cfg(any(feature = "quic", feature = "webtransport"))]
fn quic_endpoints(
    alpn: Vec<Vec<u8>>,
) -> anyhow::Result<(core::net::SocketAddr, quinn::Endpoint, quinn::Endpoint)> {
    use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
    use quinn::{ClientConfig, ServerConfig};

    let mut clt_ep = quinn::Endpoint::client((Ipv6Addr::LOCALHOST, 0).into())
        .context("failed to create client endpoint")?;

    let (mut srv_cnf, mut clt_cnf) = cert_pair().context("failed to generate certificates")?;
    srv_cnf.alpn_protocols.clone_from(&alpn);
    clt_cnf.alpn_protocols = alpn;

    let clt_cnf: QuicClientConfig = clt_cnf
        .try_into()
        .context("failed to convert rustls client config to QUIC client config")?;
    let srv_cnf: QuicServerConfig = srv_cnf
        .try_into()
        .context("failed to convert rustls server config to QUIC server config")?;

    clt_ep.set_default_client_config(ClientConfig::new(std::sync::Arc::new(clt_cnf)));
    let srv_ep = quinn::Endpoint::server(
        ServerConfig::with_crypto(std::sync::Arc::new(srv_cnf)),
        (Ipv6Addr::LOCALHOST, 0).into(),
    )
    .context("failed to create server endpoint")?;
    let srv_addr = srv_ep
        .local_addr()
        .context("failed to query listener local address")?;
    Ok((srv_addr, clt_ep, srv_ep))
}

#[cfg(feature = "quic")]
pub async fn with_quic_endpoints<T, Fut>(
    f: impl FnOnce(core::net::SocketAddr, quinn::Endpoint, quinn::Endpoint) -> Fut,
) -> anyhow::Result<T>
where
    Fut: Future<Output = anyhow::Result<T>>,
{
    let (srv_addr, clt_ep, srv_ep) = quic_endpoints(Vec::default())?;
    f(srv_addr, clt_ep, srv_ep).await.context("closure failed")
}

#[cfg(feature = "quic")]
pub async fn with_quic<T, Fut>(
    f: impl FnOnce(quinn::Connection, quinn::Connection) -> Fut,
) -> anyhow::Result<T>
where
    Fut: Future<Output = anyhow::Result<T>>,
{
    with_quic_endpoints(|addr, clt, srv| async move {
        let (clt, srv) = try_join!(
            async move {
                let conn = clt
                    .connect(addr, "::1")
                    .context("failed to connect to server")?;
                conn.await.context("failed to establish client connection")
            },
            async move {
                let conn = srv.accept().await.context("failed to accept connection")?;
                conn.await.context("failed to establish server connection")
            }
        )?;
        f(clt, srv).await.context("closure failed")
    })
    .await
}

#[cfg(feature = "webtransport")]
pub async fn with_webtransport<T, Fut>(
    f: impl FnOnce(web_transport_quinn::Session, web_transport_quinn::Session) -> Fut,
) -> anyhow::Result<T>
where
    Fut: Future<Output = anyhow::Result<T>>,
{
    let (addr, clt, srv) = quic_endpoints(vec![web_transport_quinn::ALPN.as_bytes().to_vec()])?;
    let url = url::Url::parse(&format!("https://localhost:{}", addr.port()))
        .context("failed to construct session URL")?;
    let (clt, srv) = try_join!(
        async move {
            let conn = clt
                .connect(addr, "localhost")
                .context("failed to connect to server")?;
            let conn = conn
                .await
                .context("failed to establish client connection")?;
            web_transport_quinn::Session::connect(conn, url)
                .await
                .context("failed to establish WebTransport session")
        },
        async move {
            let conn = srv.accept().await.context("failed to accept connection")?;
            let conn = conn
                .await
                .context("failed to establish server connection")?;
            let req = web_transport_quinn::Request::accept(conn)
                .await
                .context("failed to receive session request")?;
            req.ok().await.context("failed to accept session request")
        }
    )?;
    f(clt, srv).await.context("closure failed")
}

#[cfg(feature = "http")]
pub async fn with_http2<S, B, T, Fut>(
    svc: S,
    f: impl FnOnce(http::request::Parts, hyper::client::conn::http2::SendRequest<B>) -> Fut,
) -> anyhow::Result<T>
where
    S: hyper::service::HttpService<hyper::body::Incoming> + Send,
    S::ResBody: Send + 'static,
    S::Future: Send + 'static,
    <S::ResBody as hyper::body::Body>::Data: Send + 'static,
    <S::ResBody as hyper::body::Body>::Error: Send + Sync + core::error::Error + 'static,
    B: hyper::body::Body + Send + Unpin + 'static,
    B::Data: Send + Sync + 'static,
    B::Error: Send + Sync + core::error::Error + 'static,
    Fut: Future<Output = anyhow::Result<T>>,
{
    use hyper_util::rt::tokio::WithHyperIo;
    let exe = hyper_util::rt::TokioExecutor::new();

    let lis = tcp_bind().await?;
    let addr = lis
        .local_addr()
        .context("failed to query listener local address")?;

    let ((sender, clt), (srv, addr)) = try_join!(
        async {
            let conn = tcp_connect(addr).await?;
            hyper::client::conn::http2::handshake(exe.clone(), WithHyperIo::new(conn))
                .await
                .context("failed to establish HTTP/2 connection")
        },
        async { lis.accept().await.context("failed to accept connection") }
    )?;
    assert!(addr.ip().is_loopback());

    let (parts, ()) = http::Request::new(()).into_parts();
    let mut srv = pin!(
        hyper::server::conn::http2::Builder::new(exe).serve_connection(WithHyperIo::new(srv), svc)
    );
    let clt = async {
        clt.await.context("client connection failed")?;
        anyhow::Ok(core::future::pending().await)
    };
    let res = select! {
        res = f(parts, sender) => res.context("closure failed")?,
        res = &mut srv => {
            res.context("failed to serve connection")?;
            anyhow::bail!("server connection ended before the closure")
        }
        res = clt => res?,
    };
    _ = srv.await;
    Ok(res)
}

#[cfg(feature = "http")]
pub async fn with_http_pooling<B, T, Fut>(
    f: impl FnOnce(
        http::request::Parts,
        hyper_util::client::legacy::Client<
            hyper_util::client::legacy::connect::HttpConnector<
                hyper_util::client::legacy::connect::dns::GaiResolver,
            >,
            B,
        >,
        TcpListener,
    ) -> Fut,
) -> anyhow::Result<T>
where
    B: hyper::body::Body + Send + 'static,
    B::Data: Send + Sync + 'static,
    B::Error: Send + Sync + core::error::Error + 'static,
    Fut: Future<Output = anyhow::Result<T>>,
{
    let lis = tcp_bind().await?;
    let addr = lis
        .local_addr()
        .context("failed to query listener local address")?;
    let mut conn = hyper_util::client::legacy::connect::HttpConnector::new();
    conn.set_nodelay(true);
    let clt = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build(conn);
    let (mut parts, ()) = http::Request::new(()).into_parts();
    parts.uri = format!("http://{addr}").parse().unwrap();
    f(parts, clt, lis).await.context("closure failed")
}

#[cfg(feature = "quic")]
pub async fn serve_quic<T>(
    f: impl AsyncFnOnce(wrpc_quic::Connection, &wrpc_quic::Connection) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    with_quic(|clt, srv| async move { f(clt.into(), &srv.into()).await }).await
}

#[cfg(feature = "webtransport")]
pub async fn serve_webtransport<T>(
    f: impl AsyncFnOnce(wrpc_webtransport::Session, &wrpc_webtransport::Session) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    with_webtransport(|clt, srv| async move { f(clt.into(), &srv.into()).await }).await
}

#[cfg(feature = "http")]
pub async fn serve_http2<T>(
    f: impl AsyncFnOnce(
        http::request::Parts,
        wrpc_http::Client<hyper::client::conn::http2::SendRequest<wrpc_http::OutgoingBody>>,
        &mut wrpc_http::Server,
    ) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let (svc, mut srv) = wrpc_http::channel(16);
    with_http2(svc, |parts, sender| async move {
        f(parts, wrpc_http::Client::from(sender), &mut srv).await
    })
    .await
}
