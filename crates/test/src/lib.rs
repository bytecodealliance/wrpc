use core::future::poll_fn;
use core::net::Ipv6Addr;
use core::pin::pin;
use core::task::Poll;
use core::time::Duration;

use std::process::ExitStatus;

use anyhow::{Context, ensure};
use bytes::{Bytes, BytesMut};
use futures::stream::{self, FuturesUnordered};
use futures::{Stream, StreamExt as _, TryStreamExt as _};
use rcgen::{CertifiedKey, generate_simple_self_signed};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use rustls::version::TLS13;
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};
use tokio::process::Command;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio::{select, spawn, try_join};
use tokio_util::codec::Encoder as _;
use wrpc_transport::{
    Connect, Decode, Encode, FrameEncoder, FrameRef, Header, HeaderEncoder, HeaderReadError,
    HeaderRef, invoke, serve,
};

/// Maximum number of invocations [`serve_until`] serves concurrently
pub const MAX_INVOCATIONS: usize = 64;

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
                let header = Header::read(&mut rx).await?;
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
/// its context and byte streams past the header
pub async fn accept_invocation<C, I, O, E>(
    incoming: &mut (impl Stream<Item = Result<(C, O, I), E>> + Unpin),
    instance: &str,
    func: &str,
) -> anyhow::Result<(C, O, I)>
where
    I: AsyncRead + Unpin,
    E: core::error::Error + Send + Sync + 'static,
{
    let res = incoming.next().await.context("invocation stream ended")?;
    let (cx, tx, mut rx) = res.context("failed to accept invocation")?;
    let header = Header::read(&mut rx)
        .await
        .context("failed to read invocation header")?;
    ensure!(
        header.instance == instance && header.name == func,
        "unexpected invocation of `{}.{}`",
        header.instance,
        header.name,
    );
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
    type ByteStream = core::pin::Pin<Box<dyn Stream<Item = Bytes> + Send>>;
    type Values = (Bytes, ByteStream);

    let mut srv = pin!(srv);
    let ((), cx) = try_join!(
        async {
            let (tx, rx) = clt.connect(cx).await.context("failed to connect")?;
            let s: ByteStream = Box::pin(stream::iter([Bytes::from_static(b"client->server")]));
            let ((results, s), io) = invoke(
                tx,
                rx,
                "foo",
                "bar",
                <Values as Encode>::Encoder::default(),
                (Bytes::from_static(b"testbar"), s),
                <Values as Decode>::Decoder::default(),
            )
            .await
            .context("failed to invoke `foo.bar`")?;
            assert_eq!(results, b"foo".as_slice());
            let (buf, ()) = try_join!(
                async { anyhow::Ok(s.collect::<Vec<_>>().await.concat()) },
                async { io.await.context("`foo.bar` invocation failed") },
            )?;
            assert_eq!(buf, b"server->client");
            anyhow::Ok(())
        },
        async {
            let (cx, tx, rx) = accept_invocation(&mut srv, "foo", "bar").await?;
            let (params, outgoing) = serve(tx, rx, <Values as Decode>::Decoder::default());
            let ((testbar, s), rx) = params
                .await
                .context("failed to receive `foo.bar` parameters")?;
            assert_eq!(testbar, b"testbar".as_slice());
            let results: ByteStream =
                Box::pin(stream::iter([Bytes::from_static(b"server->client")]));
            let tx = outgoing
                .send(
                    <Values as Encode>::Encoder::default(),
                    (Bytes::from_static(b"foo"), results),
                )
                .context("failed to encode `foo.bar` results")?;
            let (buf, (), ()) = try_join!(
                async { anyhow::Ok(s.collect::<Vec<_>>().await.concat()) },
                rx,
                async { tx.await.context("failed to transmit `foo.bar` results") },
            )?;
            assert_eq!(buf, b"client->server");
            anyhow::Ok(cx)
        },
    )?;
    Ok(cx)
}

/// A common test used by different transport implementations, which checks that invalid data
/// fails ingress of a server and a client, and that dropping the failed side ends the invocation
/// for the peer.
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
    let mut invalid = BytesMut::default();
    FrameEncoder
        .encode(
            FrameRef {
                path: &[1],
                data: b"x",
            },
            &mut invalid,
        )
        .context("failed to encode invalid frame")?;

    let mut srv = pin!(srv);
    let (mut tx, mut rx) = clt.connect(cx.clone()).await.context("failed to connect")?;
    let mut buf = BytesMut::default();
    HeaderEncoder
        .encode(
            HeaderRef {
                instance: "foo",
                name: "baz",
            },
            &mut buf,
        )
        .context("failed to encode invocation header")?;
    buf.extend_from_slice(&invalid);
    tx.write_all(&buf)
        .await
        .context("failed to write invalid data")?;
    tx.flush().await.context("failed to flush invalid data")?;
    let (.., srv_tx, srv_rx) = accept_invocation(&mut srv, "foo", "baz").await?;
    let (params, outgoing) = serve(srv_tx, srv_rx, <() as Decode>::Decoder::default());
    let res = params.await;
    ensure!(res.is_err(), "invalid data must fail server ingress");
    drop(outgoing);
    let mut buf = Vec::default();
    let res = timeout(Duration::from_secs(5), rx.read_to_end(&mut buf))
        .await
        .context("invalid data did not end the invocation")?;
    ensure!(
        res.is_err() || buf.is_empty(),
        "server responded to invalid data"
    );

    let (tx, rx) = clt.connect(cx).await.context("failed to connect")?;
    try_join!(
        async {
            let res = invoke(
                tx,
                rx,
                "foo",
                "baz",
                <() as Encode>::Encoder::default(),
                (),
                <() as Decode>::Decoder::default(),
            )
            .await;
            ensure!(res.is_err(), "invalid data must fail client ingress");
            anyhow::Ok(())
        },
        async {
            let (.., mut srv_tx, mut srv_rx) = accept_invocation(&mut srv, "foo", "baz").await?;
            srv_tx
                .write_all(&invalid)
                .await
                .context("failed to write invalid data")?;
            srv_tx
                .flush()
                .await
                .context("failed to flush invalid data")?;
            let mut buf = Vec::default();
            _ = timeout(Duration::from_secs(5), srv_rx.read_to_end(&mut buf))
                .await
                .context("invalid data did not end the invocation")?;
            anyhow::Ok(())
        },
    )?;
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
