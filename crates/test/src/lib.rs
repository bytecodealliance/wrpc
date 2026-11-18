use core::future::poll_fn;
use core::net::Ipv6Addr;
use core::pin::pin;
use core::task::Poll;
use core::time::Duration;

use anyhow::{Context, ensure};
use bytes::{Bytes, BytesMut};
use futures::stream::{self, BoxStream, FuturesUnordered};
use futures::{StreamExt as _, TryStreamExt as _};
use rcgen::{CertifiedKey, generate_simple_self_signed};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use rustls::version::TLS13;
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};
use tokio::time::timeout;
use tokio::try_join;
use tokio_util::codec::{Encoder as _, FramedRead};
use wrpc_transport::{
    Decode, Encode, FrameDecoder, FrameEncoder, FrameRef, Header, HeaderDecoder, HeaderEncoder,
    HeaderRef, invoke, serve,
};

/// Maximum number of connections [`serve_until`] serves concurrently
const MAX_CONNECTIONS: usize = 64;

/// Polls `fut` to completion while accepting connections on `lis` with `TCP_NODELAY` enabled and
/// polling the future `f` returns for each, then completes the served connections. At most
/// `MAX_CONNECTIONS` are served concurrently. Returns the first error.
pub async fn serve_until<T, Fut>(
    lis: TcpListener,
    fut: impl Future<Output = anyhow::Result<T>>,
    mut f: impl FnMut(OwnedWriteHalf, OwnedReadHalf) -> Fut,
) -> anyhow::Result<T>
where
    Fut: Future<Output = anyhow::Result<()>>,
{
    let mut fut = pin!(fut);
    let mut served = FuturesUnordered::default();
    let res = poll_fn(|cx| {
        while served.len() < MAX_CONNECTIONS
            && let Poll::Ready(res) = lis.poll_accept(cx)
        {
            let (stream, ..) = res.context("failed to accept connection")?;
            stream
                .set_nodelay(true)
                .context("failed to set `TCP_NODELAY`")?;
            let (rx, tx) = stream.into_split();
            served.push(f(tx, rx));
        }
        while let Poll::Ready(Some(res)) = served.poll_next_unpin(cx) {
            res?;
            cx.waker().wake_by_ref();
        }
        fut.as_mut().poll(cx)
    })
    .await?;
    served.try_collect::<()>().await?;
    Ok(res)
}

/// Reads the header of the next invocation from `rx`, ensures that it invokes function `func` on
/// instance `instance` and returns the frames following it
pub async fn read_invocation<I>(
    mut rx: FramedRead<I, HeaderDecoder>,
    instance: &str,
    func: &str,
) -> anyhow::Result<FramedRead<I, FrameDecoder>>
where
    I: AsyncRead + Unpin,
{
    let header = rx
        .try_next()
        .await
        .context("failed to read invocation header")?;
    let Header {
        instance: inst,
        name,
    } = header.context("byte stream ended")?;
    ensure!(
        inst == instance && name == func,
        "unexpected invocation of `{inst}.{name}`",
    );
    Ok(rx.map_decoder(|_| FrameDecoder::default()))
}

/// Reads `rx` to EOF, ensuring that no frames are left
pub async fn read_eof(
    rx: &mut FramedRead<impl AsyncRead + Unpin, FrameDecoder>,
) -> anyhow::Result<()> {
    let frame = rx.try_next().await.context("failed to read EOF")?;
    ensure!(frame.is_none(), "unexpected frame after invocation");
    Ok(())
}

pub async fn serve_tcp<T>(
    f: impl AsyncFnOnce(core::net::SocketAddr, TcpListener) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let lis = tcp_bind().await?;
    let addr = lis
        .local_addr()
        .context("failed to query listener local address")?;
    f(addr, lis).await
}

#[cfg(all(unix, feature = "net"))]
pub async fn serve_unix<T>(
    f: impl AsyncFnOnce(std::path::PathBuf, tokio::net::UnixListener) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let dir = tempfile::tempdir().context("failed to create temporary directory")?;
    let path = dir.path().join("sock");
    let lis = tokio::net::UnixListener::bind(&path).context("failed to bind Unix socket")?;
    f(path, lis).await
}

/// Connects to `addr` with `TCP_NODELAY` enabled, returning the byte streams
pub async fn tcp_streams(
    addr: impl ToSocketAddrs,
) -> std::io::Result<(OwnedWriteHalf, OwnedReadHalf)> {
    let stream = tcp_connect(addr).await?;
    let (rx, tx) = stream.into_split();
    Ok((tx, rx))
}

/// Connects to the Unix socket at `path`, returning the byte streams
#[cfg(unix)]
pub async fn unix_streams(
    path: std::path::PathBuf,
) -> std::io::Result<(
    tokio::net::unix::OwnedWriteHalf,
    tokio::net::unix::OwnedReadHalf,
)> {
    let stream = tokio::net::UnixStream::connect(path).await?;
    let (rx, tx) = stream.into_split();
    Ok((tx, rx))
}

/// Accepts a connection on `lis` with `TCP_NODELAY` enabled, returning the byte streams
pub async fn tcp_accept(lis: &TcpListener) -> std::io::Result<(OwnedWriteHalf, OwnedReadHalf)> {
    let (stream, ..) = lis.accept().await?;
    stream.set_nodelay(true)?;
    let (rx, tx) = stream.into_split();
    Ok((tx, rx))
}

/// Accepts a connection on `lis`, returning the byte streams
#[cfg(unix)]
pub async fn unix_accept(
    lis: &tokio::net::UnixListener,
) -> std::io::Result<(
    tokio::net::unix::OwnedWriteHalf,
    tokio::net::unix::OwnedReadHalf,
)> {
    let (stream, ..) = lis.accept().await?;
    let (rx, tx) = stream.into_split();
    Ok((tx, rx))
}

pub async fn tcp_bind() -> anyhow::Result<TcpListener> {
    TcpListener::bind((Ipv6Addr::LOCALHOST, 0))
        .await
        .context("failed to start TCP listener")
}

/// Connects to `addr` with `TCP_NODELAY` enabled
pub async fn tcp_connect(addr: impl ToSocketAddrs) -> std::io::Result<TcpStream> {
    let stream = TcpStream::connect(addr).await?;
    stream.set_nodelay(true)?;
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
/// The invocation is made on `tx` and `rx` and accepted by `accept`.
pub async fn assert_single_invocation(
    tx: impl AsyncWrite + Send + Unpin,
    rx: impl AsyncRead + Send + Unpin,
    accept: impl Future<
        Output = Result<
            (
                impl AsyncWrite + Send + Unpin + 'static,
                impl AsyncRead + Send + Unpin + 'static,
            ),
            impl core::error::Error + Send + Sync + 'static,
        >,
    >,
) -> anyhow::Result<()> {
    try_join!(
        async {
            let s: BoxStream<'static, Bytes> =
                Box::pin(stream::iter([Bytes::from_static(b"client->server")]));
            let ((results, s), io) = invoke(
                tx,
                rx,
                "foo",
                "bar",
                <(Bytes, BoxStream<'static, Bytes>) as Encode>::Encoder::default(),
                (Bytes::from_static(b"testbar"), s),
                <(Bytes, BoxStream<'static, Bytes>) as Decode>::Decoder::default(),
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
            let (tx, rx) = accept.await.context("failed to accept")?;
            let mut frames =
                read_invocation(FramedRead::new(rx, HeaderDecoder::default()), "foo", "bar")
                    .await?;
            let (params, outgoing) = serve(
                tx,
                &mut frames,
                <(Bytes, BoxStream<'static, Bytes>) as Decode>::Decoder::default(),
            );
            let ((testbar, s), rx) = params
                .await
                .context("failed to receive `foo.bar` parameters")?;
            assert_eq!(testbar, b"testbar".as_slice());
            let results: BoxStream<'static, Bytes> =
                Box::pin(stream::iter([Bytes::from_static(b"server->client")]));
            let tx = outgoing
                .send(
                    <(Bytes, BoxStream<'static, Bytes>) as Encode>::Encoder::default(),
                    (Bytes::from_static(b"foo"), results),
                )
                .context("failed to encode `foo.bar` results")?;
            let (buf, (), ()) = try_join!(
                async { anyhow::Ok(s.collect::<Vec<_>>().await.concat()) },
                async { rx.await.context("failed to receive `foo.bar` parameters") },
                async { tx.await.context("failed to transmit `foo.bar` results") },
            )?;
            assert_eq!(buf, b"client->server");
            read_eof(&mut frames).await?;
            anyhow::Ok(())
        },
    )?;
    Ok(())
}

/// A common test used by different transport implementations, which checks that invalid data
/// fails ingress of a server and a client, and that dropping the failed side ends the invocation
/// for the peer.
///
/// The invocations are accepted using `accept`.
pub async fn assert_invalid_data_resets<CI, CO, CE, I, O, E>(
    connect: impl AsyncFn() -> Result<(CO, CI), CE>,
    mut accept: impl AsyncFnMut() -> Result<(O, I), E>,
) -> anyhow::Result<()>
where
    CI: AsyncRead + Send + Unpin,
    CO: AsyncWrite + Send + Unpin,
    CE: core::error::Error + Send + Sync + 'static,
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
    let ((_tx, mut rx), (srv_tx, srv_rx)) = try_join!(
        async {
            let (mut tx, rx) = connect().await.context("failed to connect")?;
            tx.write_all(&buf)
                .await
                .context("failed to write invalid data")?;
            tx.flush().await.context("failed to flush invalid data")?;
            anyhow::Ok((tx, rx))
        },
        async {
            let (tx, rx) = accept().await.context("failed to accept")?;
            let rx = read_invocation(FramedRead::new(rx, HeaderDecoder::default()), "foo", "baz")
                .await?;
            anyhow::Ok((tx, rx))
        },
    )?;
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

    try_join!(
        async {
            let (tx, rx) = connect().await.context("failed to connect")?;
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
            let (mut srv_tx, srv_rx) = accept().await.context("failed to accept")?;
            let srv_rx = read_invocation(
                FramedRead::new(srv_rx, HeaderDecoder::default()),
                "foo",
                "baz",
            )
            .await?;
            srv_tx
                .write_all(&invalid)
                .await
                .context("failed to write invalid data")?;
            srv_tx
                .flush()
                .await
                .context("failed to flush invalid data")?;
            _ = timeout(Duration::from_secs(5), srv_rx.count())
                .await
                .context("invalid data did not end the invocation")?;
            anyhow::Ok(())
        },
    )?;
    Ok(())
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
pub async fn with_quic<T, Fut>(
    f: impl FnOnce(quinn::Connection, quinn::Connection) -> Fut,
) -> anyhow::Result<T>
where
    Fut: Future<Output = anyhow::Result<T>>,
{
    let (addr, clt, srv) = quic_endpoints(Vec::default())?;
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
    let res = tokio::select! {
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

/// Accepts a request from `srv`, returning the byte streams
#[cfg(feature = "http")]
pub async fn http_accept(
    srv: &mut wrpc_http::Server,
) -> std::io::Result<(tokio::io::DuplexStream, wrpc_http::IncomingBodyDataReader)> {
    match srv.next().await {
        Some(Ok((.., tx, rx))) => Ok((tx, rx)),
        None => Err(std::io::ErrorKind::UnexpectedEof.into()),
    }
}

#[cfg(feature = "http")]
pub async fn serve_http2<T>(
    f: impl AsyncFnOnce(
        http::request::Parts,
        hyper::client::conn::http2::SendRequest<wrpc_http::OutgoingBody>,
        &mut wrpc_http::Server,
    ) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let (svc, mut srv) = wrpc_http::channel(16);
    with_http2(svc, |parts, sender| async move {
        f(parts, sender, &mut srv).await
    })
    .await
}
