use core::future::poll_fn;
use core::net::Ipv6Addr;
use core::pin::pin;
use core::task::Poll;

use std::process::ExitStatus;
use std::sync::Arc;

use anyhow::Context;
use futures::TryStreamExt as _;
use rcgen::{CertifiedKey, generate_simple_self_signed};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use rustls::version::TLS13;
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};
use tokio::process::Command;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::{select, spawn, try_join};
use tracing::info;
use wrpc_transport::{Invoke, Serve};

/// Polls `fut` to completion while repeatedly running `accept` to feed established connections
/// into a server, returning the first error of either.
pub async fn accept_until<T>(
    fut: impl Future<Output = anyhow::Result<T>>,
    accept: impl AsyncFn() -> anyhow::Result<()>,
) -> anyhow::Result<T> {
    let mut fut = pin!(fut);
    let mut accepting = pin!(accept());
    poll_fn(|cx| {
        while let Poll::Ready(res) = accepting.as_mut().poll(cx) {
            res?;
            accepting.set(accept());
        }
        fut.as_mut().poll(cx)
    })
    .await
}

pub async fn accept_tcp(
    srv: &wrpc_transport::Server<
        (),
        BufReader<wrpc_transport::tcp::ReadHalf>,
        wrpc_transport::tcp::WriteHalf,
    >,
    lis: &TcpListener,
) -> anyhow::Result<()> {
    let (stream, ..) = lis.accept().await.context("failed to accept connection")?;
    let (rx, tx) = wrpc_transport::tcp::split(stream);
    srv.accept((), tx, BufReader::new(rx))
        .await
        .context("failed to accept invocation")
}

#[cfg(unix)]
pub async fn accept_unix(
    srv: &wrpc_transport::Server<
        (),
        BufReader<wrpc_transport::unix::ReadHalf>,
        wrpc_transport::unix::WriteHalf,
    >,
    lis: &tokio::net::UnixListener,
) -> anyhow::Result<()> {
    let (stream, ..) = lis.accept().await.context("failed to accept connection")?;
    let (rx, tx) = wrpc_transport::unix::split(stream);
    srv.accept((), tx, BufReader::new(rx))
        .await
        .context("failed to accept invocation")
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
/// The server is set up via the [`Serve`] implementation `srv`. The `accept`
/// future is run concurrently and is responsible for feeding established
/// connections into `srv` (e.g. accepting a bidirectional stream); transports
/// that subscribe internally can pass a future that resolves immediately.
pub async fn assert_single_invocation<C, S>(
    cx: C::Context,
    clt: &C,
    srv: &S,
    accept: impl Future<Output = anyhow::Result<()>>,
) -> anyhow::Result<S::Context>
where
    C: Invoke,
    S: Serve,
{
    let invocations = srv
        .serve("foo", "bar", Arc::from([Box::from([Some(42), Some(0)])]))
        .await
        .context("failed to serve `foo.bar`")?;
    let mut invocations = pin!(invocations);
    let ((), cx) = try_join!(
        async {
            let (mut outgoing, mut incoming) = clt
                .invoke(cx, "foo", "bar", "test".into(), &[&[Some(0), Some(42)]])
                .await
                .context("failed to invoke `foo.bar`")?;
            let mut nested_tx = outgoing.index(&[42, 0]).context("failed to index `42.0`")?;
            let mut nested_rx = incoming.index(&[0, 42]).context("failed to index `0.42`")?;
            try_join!(
                async {
                    info!("reading `foo`");
                    let mut buf = vec![];
                    let n = incoming
                        .read_to_end(&mut buf)
                        .await
                        .context("failed to read `foo`")?;
                    assert_eq!(n, 3);
                    assert_eq!(buf, b"foo");
                    info!("read `foo`");
                    anyhow::Ok(())
                },
                async {
                    info!("writing `bar`");
                    outgoing
                        .write_all(b"bar")
                        .await
                        .context("failed to write `bar`")?;
                    drop(outgoing);
                    info!("wrote `bar`");
                    anyhow::Ok(())
                },
                async {
                    info!("writing `client->server`");
                    nested_tx
                        .write_all(b"client->server")
                        .await
                        .context("failed to write `client->server`")?;
                    drop(nested_tx);
                    info!("wrote `client->server`");
                    anyhow::Ok(())
                },
                async {
                    info!("reading `server->client`");
                    let mut buf = vec![];
                    nested_rx
                        .read_to_end(&mut buf)
                        .await
                        .context("failed to read `server->client`")?;
                    assert_eq!(buf, b"server->client");
                    info!("read `server->client`");
                    anyhow::Ok(())
                },
            )?;
            anyhow::Ok(())
        },
        async {
            let ((), (cx, mut outgoing, mut incoming)) = try_join!(accept, async {
                invocations
                    .try_next()
                    .await
                    .context("failed to accept invocation")?
                    .context("invocation stream unexpectedly finished")
            })?;
            let mut nested_tx = outgoing.index(&[0, 42]).context("failed to index `0.42`")?;
            let mut nested_rx = incoming.index(&[42, 0]).context("failed to index `42.0`")?;
            try_join!(
                async {
                    info!("reading `test`");
                    let mut buf = vec![0; 4];
                    incoming
                        .read_exact(&mut buf)
                        .await
                        .context("failed to read `test`")?;
                    assert_eq!(buf, b"test");
                    info!("read `test`");

                    info!("reading `bar`");
                    let mut buf = vec![];
                    let n = incoming
                        .read_to_end(&mut buf)
                        .await
                        .context("failed to read `bar`")?;
                    assert_eq!(n, 3);
                    assert_eq!(buf, b"bar");
                    info!("read `bar`");
                    anyhow::Ok(())
                },
                async {
                    info!("writing `foo`");
                    outgoing
                        .write_all(b"foo")
                        .await
                        .context("failed to write `foo`")?;
                    drop(outgoing);
                    info!("wrote `foo`");
                    anyhow::Ok(())
                },
                async {
                    info!("writing `server->client`");
                    nested_tx
                        .write_all(b"server->client")
                        .await
                        .context("failed to write `server->client`")?;
                    drop(nested_tx);
                    info!("wrote `server->client`");
                    anyhow::Ok(())
                },
                async {
                    info!("reading `client->server`");
                    let mut buf = vec![];
                    let n = nested_rx
                        .read_to_end(&mut buf)
                        .await
                        .context("failed to read `client->server`")?;
                    assert_eq!(n, 14);
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

    clt_ep.set_default_client_config(ClientConfig::new(Arc::new(clt_cnf)));
    let srv_ep = quinn::Endpoint::server(
        ServerConfig::with_crypto(Arc::new(srv_cnf)),
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

#[cfg(feature = "quic")]
pub async fn accept_quic<I, O>(
    srv: &wrpc_transport::Server<(), I, O>,
    conn: &quinn::Connection,
) -> anyhow::Result<()>
where
    I: From<quinn::RecvStream> + tokio::io::AsyncBufRead + Unpin + Send + Sync + 'static,
    O: From<quinn::SendStream> + Send + Sync + 'static,
{
    let (tx, rx) = conn.accept_bi().await.context("failed to accept stream")?;
    srv.accept((), tx.into(), rx.into())
        .await
        .context("failed to accept invocation")
}

#[cfg(feature = "webtransport")]
pub async fn accept_webtransport<I, O>(
    srv: &wrpc_transport::Server<(), I, O>,
    session: &web_transport_quinn::Session,
) -> anyhow::Result<()>
where
    I: From<web_transport_quinn::RecvStream>
        + tokio::io::AsyncBufRead
        + Unpin
        + Send
        + Sync
        + 'static,
    O: From<web_transport_quinn::SendStream> + Send + Sync + 'static,
{
    let (tx, rx) = session
        .accept_bi()
        .await
        .context("failed to accept stream")?;
    srv.accept((), tx.into(), rx.into())
        .await
        .context("failed to accept invocation")
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
    let (res, (), ()) = try_join!(
        async { f(parts, sender).await.context("closure failed") },
        async {
            hyper::server::conn::http2::Builder::new(exe)
                .serve_connection(WithHyperIo::new(srv), svc)
                .await
                .context("failed to serve connection")
        },
        async { clt.await.context("client connection failed") }
    )?;
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

#[cfg(feature = "websockets")]
pub async fn with_websockets<T, Fut>(
    f: impl FnOnce(
        tokio_websockets::ClientBuilder<'static>,
        tokio_websockets::ServerBuilder,
        TcpListener,
    ) -> Fut,
) -> anyhow::Result<T>
where
    Fut: Future<Output = anyhow::Result<T>>,
{
    use tokio_websockets::{ClientBuilder, ServerBuilder};

    let lis = tcp_bind().await?;
    let addr = lis
        .local_addr()
        .context("failed to query listener local address")?;
    let clt = ClientBuilder::new()
        .uri(&format!("ws://{addr}"))
        .context("failed to set WebSocket URI")?;
    f(clt, ServerBuilder::new(), lis)
        .await
        .context("closure failed")
}

#[cfg(feature = "websockets")]
pub async fn accept_websocket_stream(
    lis: &TcpListener,
    srv: &tokio_websockets::ServerBuilder,
) -> anyhow::Result<tokio_websockets::WebSocketStream<TcpStream>> {
    let (stream, addr) = lis
        .accept()
        .await
        .context("failed to accept TCP connection")?;
    anyhow::ensure!(addr.ip().is_loopback(), "unexpected peer address `{addr}`");
    let (_req, ws) = srv
        .accept(stream)
        .await
        .context("failed to perform WebSocket handshake")?;
    Ok(ws)
}

#[cfg(feature = "websockets")]
pub async fn accept_websocket<I, O>(
    srv: &wrpc_transport::Server<(), I, O>,
    lis: &TcpListener,
    sb: &tokio_websockets::ServerBuilder,
    split: impl FnOnce(tokio_websockets::WebSocketStream<TcpStream>) -> (O, I),
) -> anyhow::Result<()>
where
    I: tokio::io::AsyncBufRead + Unpin + Send + Sync + 'static,
    O: Send + Sync + 'static,
{
    let ws = accept_websocket_stream(lis, sb).await?;
    let (tx, rx) = split(ws);
    srv.accept((), tx, rx)
        .await
        .context("failed to accept invocation")
}
