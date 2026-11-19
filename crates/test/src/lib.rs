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
use tokio_util::codec::{Encoder as _, FramedRead, FramedWrite};
use wrpc_transport::{
    Decode, FrameDecoder, FrameEncoder, FrameRef, Header, HeaderEncoder, HeaderRef, Listener,
    invoke, receive, serve,
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

/// Receives the header of the next invocation over `lis`, ensures that it invokes function `func`
/// on instance `instance` and returns the frames to serve it over
pub async fn next_invocation<'a, I, O>(
    lis: &'a mut Listener<I, O>,
    instance: &str,
    func: &str,
) -> anyhow::Result<(
    &'a mut FramedWrite<O, FrameEncoder>,
    &'a mut FramedRead<I, FrameDecoder>,
)>
where
    I: AsyncRead + Unpin,
{
    let next = lis
        .accept()
        .await
        .context("failed to read invocation header")?;
    let (
        Header {
            instance: inst,
            name,
        },
        tx,
        rx,
    ) = next.context("byte stream ended")?;
    ensure!(
        inst == instance && name == func,
        "unexpected invocation of `{inst}.{name}`",
    );
    Ok((tx, rx))
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
            let tx = FramedWrite::new(tx, FrameEncoder);
            let mut rx = FramedRead::new(rx, FrameDecoder::default());
            let ((results, s), io) = invoke::<_, (Bytes, BoxStream<'static, Bytes>), _, _, _, _>(
                tx,
                &mut rx,
                "foo",
                "bar",
                (Bytes::from_static(b"testbar"), s),
            )
            .await
            .context("failed to invoke `foo.bar`")?;
            assert_eq!(results, b"foo".as_slice());
            let (buf, ()) = try_join!(
                async { anyhow::Ok(s.collect::<Vec<_>>().await.concat()) },
                async { io.await.context("`foo.bar` invocation failed") },
            )?;
            assert_eq!(buf, b"server->client");
            read_eof(&mut rx).await?;
            anyhow::Ok(())
        },
        async {
            let (tx, rx) = accept.await.context("failed to accept")?;
            let mut lis = Listener::new(tx, rx);
            let (tx, rx) = next_invocation(&mut lis, "foo", "bar").await?;
            serve::<
                (Bytes, BoxStream<'static, Bytes>),
                (Bytes, BoxStream<'static, Bytes>),
                _,
                _,
                _,
                _,
            >(tx, rx, async |(testbar, s)| {
                assert_eq!(testbar, b"testbar".as_slice());
                let buf = s.collect::<Vec<_>>().await.concat();
                assert_eq!(buf, b"client->server");
                let results: BoxStream<'static, Bytes> =
                    Box::pin(stream::iter([Bytes::from_static(b"server->client")]));
                anyhow::Ok((Bytes::from_static(b"foo"), results))
            })
            .await
            .context("failed to serve `foo.bar`")?;
            let next = lis.accept().await?;
            ensure!(next.is_none(), "unexpected invocation");
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
    let ((_tx, mut rx), mut lis) = try_join!(
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
            anyhow::Ok(Listener::new(tx, rx))
        },
    )?;
    let (_, srv_rx) = next_invocation(&mut lis, "foo", "baz").await?;
    let res = receive(<() as Decode>::Decoder::default(), srv_rx).await;
    ensure!(res.is_err(), "invalid data must fail server ingress");
    drop(lis);
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
            let tx = FramedWrite::new(tx, FrameEncoder);
            let rx = FramedRead::new(rx, FrameDecoder::default());
            let res = invoke::<(), (), _, _, _, _>(tx, rx, "foo", "baz", ()).await;
            ensure!(res.is_err(), "invalid data must fail client ingress");
            anyhow::Ok(())
        },
        async {
            let (tx, rx) = accept().await.context("failed to accept")?;
            let mut lis = Listener::new(tx, rx);
            let (srv_tx, srv_rx) = next_invocation(&mut lis, "foo", "baz").await?;
            let srv_tx = srv_tx.get_mut();
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
pub async fn with_http2<B, T, Fut>(
    serve: impl AsyncFnOnce(TcpStream) -> anyhow::Result<()>,
    f: impl FnOnce(http::request::Parts, hyper::client::conn::http2::SendRequest<B>) -> Fut,
) -> anyhow::Result<T>
where
    B: hyper::body::Body + Send + Unpin + 'static,
    B::Data: Send + Sync + 'static,
    B::Error: Send + Sync + core::error::Error + 'static,
    Fut: Future<Output = anyhow::Result<T>>,
{
    use hyper_util::rt::tokio::WithHyperIo;

    let lis = tcp_bind().await?;
    let addr = lis
        .local_addr()
        .context("failed to query listener local address")?;

    let ((sender, clt), (srv, addr)) = try_join!(
        async {
            let conn = tcp_connect(addr).await?;
            hyper::client::conn::http2::handshake(
                hyper_util::rt::TokioExecutor::default(),
                WithHyperIo::new(conn),
            )
            .await
            .context("failed to establish HTTP/2 connection")
        },
        async { lis.accept().await.context("failed to accept connection") }
    )?;
    assert!(addr.ip().is_loopback());

    let (parts, ()) = http::Request::new(()).into_parts();
    let mut srv = pin!(serve(srv));
    let clt = async {
        clt.await.context("client connection failed")?;
        anyhow::Ok(core::future::pending().await)
    };
    let res = tokio::select! {
        res = f(parts, sender) => res.context("closure failed")?,
        res = &mut srv => {
            res?;
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

/// Receives the byte streams of a request from `rx`
#[cfg(feature = "http")]
pub async fn http_accept(
    rx: &mut tokio::sync::mpsc::Receiver<(
        tokio::io::DuplexStream,
        wrpc_http::RequestBodyReader<hyper::body::Incoming>,
    )>,
) -> std::io::Result<(
    tokio::io::DuplexStream,
    wrpc_http::RequestBodyReader<hyper::body::Incoming>,
)> {
    rx.recv()
        .await
        .ok_or_else(|| std::io::ErrorKind::UnexpectedEof.into())
}

/// Serves HTTP/1 connection `stream`, sending the byte streams of each request to `tx`
#[cfg(feature = "http")]
pub async fn serve_http1(
    stream: TcpStream,
    tx: tokio::sync::mpsc::Sender<(
        tokio::io::DuplexStream,
        wrpc_http::RequestBodyReader<hyper::body::Incoming>,
    )>,
) -> anyhow::Result<()> {
    hyper::server::conn::http1::Builder::new()
        .serve_connection(
            hyper_util::rt::TokioIo::new(stream),
            hyper::service::service_fn(move |req| {
                let (res, body_tx, body_rx) = wrpc_http::accept(req.into_body());
                let tx = tx.clone();
                async move {
                    tx.send((body_tx, body_rx))
                        .await
                        .map_err(|_| std::io::Error::from(std::io::ErrorKind::BrokenPipe))?;
                    Ok::<_, std::io::Error>(res)
                }
            }),
        )
        .await
        .context("failed to serve HTTP/1 connection")
}

#[cfg(feature = "http")]
pub async fn serve_http2<T>(
    f: impl AsyncFnOnce(
        http::request::Parts,
        hyper::client::conn::http2::SendRequest<wrpc_http::OutgoingBody>,
        &mut tokio::sync::mpsc::Receiver<(
            tokio::io::DuplexStream,
            wrpc_http::RequestBodyReader<hyper::body::Incoming>,
        )>,
    ) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    with_http2(
        async |stream| {
            wrpc_http::serve(stream, move |_, body_tx, body_rx| {
                tx.try_send((body_tx, body_rx))
                    .expect("failed to send request byte streams");
            })
            .await
            .context("failed to serve HTTP/2 connection")
        },
        |parts, sender| async move { f(parts, sender, &mut rx).await },
    )
    .await
}
