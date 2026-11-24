use core::str::Utf8Error;
use core::time::Duration;

use std::io::ErrorKind;
use std::rc::Rc;

use anyhow::{Context as _, bail, ensure};
use bytes::{Bytes, BytesMut};
use futures::StreamExt as _;
use futures::channel::{mpsc, oneshot};
use futures::future::{self, BoxFuture, LocalBoxFuture};
use futures::stream::{self, BoxStream};
use http::header::CONTENT_TYPE;
use http::{HeaderValue, StatusCode, Uri};
use http_body::Body as _;
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::client::conn::http2::{SendRequest, handshake};
use hyper::server::conn::http2::Builder;
use hyper::service::{Service, service_fn};
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::io::duplex;
use tokio::time::sleep;
use tokio::{select, spawn, try_join};
use tokio_util::codec::Encoder as _;
use wrpc_core::{
    Codec, Decode, Encode, FrameDecodeError, FrameEncoder, FrameRef, Header, HeaderEncoder,
    HeaderRef, InvokeError, ReceiveError, ServeError, Utf8Bytes, invoke, serve,
};
use wrpc_http::{Client, IncomingRequest, OutgoingParams};

type BoxBody = UnsyncBoxBody<Bytes, std::io::Error>;

fn service<F, Fut>(
    f: F,
    tx: mpsc::UnboundedSender<anyhow::Result<()>>,
) -> impl Service<
    http::Request<Incoming>,
    Response = http::Response<BoxBody>,
    Error = anyhow::Error,
    Future: Send + 'static,
> + Clone
+ Send
+ 'static
where
    F: Fn(Header, IncomingRequest<Incoming>) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<http::Response<BoxBody>, ServeError>> + Send + 'static,
{
    service_fn(move |req: http::Request<Incoming>| {
        let f = f.clone();
        let tx = tx.clone();
        async move {
            let res = async {
                let (header, req) = wrpc_http::accept(req.into_body()).await?;
                let res = f(header, req).await?;
                anyhow::Ok(res)
            };
            match res.await {
                Ok(res) => {
                    _ = tx.unbounded_send(Ok(()));
                    Ok(res)
                }
                Err(err) => {
                    _ = tx.unbounded_send(Err(err));
                    bail!("failed to serve invocation")
                }
            }
        }
    })
}

async fn connect<B, F, Fut>(
    f: F,
) -> anyhow::Result<(SendRequest<B>, mpsc::UnboundedReceiver<anyhow::Result<()>>)>
where
    B: http_body::Body<Data = Bytes, Error = std::io::Error> + Send + Unpin + 'static,
    F: Fn(Header, IncomingRequest<Incoming>) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<http::Response<BoxBody>, ServeError>> + Send + 'static,
{
    let (tx, results) = mpsc::unbounded();
    let (clt, srv) = duplex(65536);
    let (sender, conn) = handshake(TokioExecutor::default(), TokioIo::new(clt)).await?;
    spawn(conn);
    spawn(
        Builder::new(TokioExecutor::default()).serve_connection(TokioIo::new(srv), service(f, tx)),
    );
    Ok((sender, results))
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn http2() -> anyhow::Result<()> {
    let mut baz = BytesMut::default();
    HeaderEncoder.encode(
        HeaderRef {
            instance: "foo",
            name: "baz",
        },
        &mut baz,
    )?;
    for path in [&[1][..], &[]] {
        let mut buf = baz.clone();
        FrameEncoder.encode(FrameRef { path, data: b"" }, &mut buf)?;
        buf.extend_from_slice(&baz);
        let (mut sender, mut results) = connect(|Header { name, .. }, req| {
            serve::<(), (), _, _>(req, async move |()| {
                if name != "baz" {
                    return Err(std::io::Error::other(format!("unexpected `{name}`")));
                }
                Ok(())
            })
        })
        .await?;
        let body = Full::new(buf.freeze()).map_err(|err| match err {});
        let req = http::Request::new(body.boxed_unsync());
        sender.ready().await?;
        let res = async {
            let res = sender.send_request(req).await?;
            let body = res.into_body().collect().await?;
            anyhow::Ok(body)
        };
        res.await
            .expect_err("invalid data must fail the invocation");
        let res = results.next().await.context("missing invocation")?;
        match res {
            Err(err) if matches!(err.downcast_ref(), Some(ServeError::Request(..))) => {}
            Ok(()) if path.is_empty() => {}
            res => bail!("unexpected result `{res:?}`"),
        }
    }
    Ok(())
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn unread_params() -> anyhow::Result<()> {
    let (mut sender, mut results) = connect(|_, req| {
        serve::<(BoxStream<'static, Bytes>,), (), _, _>(req, async |(s,)| {
            drop(s);
            Ok(())
        })
    })
    .await?;
    let s: BoxStream<'static, Bytes> = Box::pin(stream::repeat(Bytes::from_static(b"foo")).then(
        async |buf| {
            sleep(Duration::from_millis(10)).await;
            buf
        },
    ));
    sender.ready().await?;
    let clt = Client::new(
        Uri::default(),
        tower::util::service_fn(|req| sender.send_request(req)),
    );
    let res = async {
        let ((), io) = invoke::<_, (), _>(clt, "foo", "bar", (s,)).await?;
        io.await
    };
    res.await
        .expect_err("unread parameters must fail the invocation");
    results.next().await.context("missing invocation")??;
    Ok(())
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn peer_cancel() -> anyhow::Result<()> {
    struct Done(mpsc::UnboundedSender<()>);

    impl Drop for Done {
        fn drop(&mut self) {
            _ = self.0.unbounded_send(());
        }
    }

    let (done_tx, mut done_rx) = mpsc::unbounded();
    let (started_tx, mut started_rx) = mpsc::unbounded();
    let (mut sender, mut results) = connect(move |header, req| {
        let done = Done(done_tx.clone());
        let started_tx = started_tx.clone();
        async move {
            if header.name != "stream" {
                return wrpc_test::serve_http_invocation(header, req)
                    .await
                    .map_err(|err| ServeError::Handler(std::io::Error::other(err)));
            }
            let _done = done;
            serve::<(BoxStream<'static, Bytes>,), (), _, _>(req, async |(_s,)| {
                started_tx
                    .unbounded_send(())
                    .map_err(std::io::Error::other)?;
                future::pending().await
            })
            .await
        }
    })
    .await?;
    let s: BoxStream<'static, Bytes> = Box::pin(stream::repeat(Bytes::from_static(b"foo")).then(
        async |buf| {
            sleep(Duration::from_millis(10)).await;
            buf
        },
    ));
    sender.ready().await?;
    let clt = Client::new(
        Uri::default(),
        tower::util::service_fn(|req| sender.send_request(req)),
    );
    select! {
        _ = invoke::<_, (), _>(clt, "foo", "stream", (s,)) => {
            bail!("invocation must not complete");
        }
        _ = started_rx.next() => {}
    }
    done_rx.next().await.context("invocation was not served")?;
    let res = results.next().await.context("missing invocation")?;
    let Some(ServeError::Request(ReceiveError::Frame(FrameDecodeError::Io(err)))) =
        res.as_ref().err().and_then(|err| err.downcast_ref())
    else {
        bail!("unexpected result `{res:?}`");
    };
    assert_eq!(err.kind(), ErrorKind::ConnectionReset);
    sender.ready().await?;
    wrpc_test::assert_http_invocation(Client::new(
        Uri::default(),
        tower::util::service_fn(|req| sender.send_request(req)),
    ))
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn pooling() -> anyhow::Result<()> {
    wrpc_test::with_http_pooling(|uri, clt, lis| async move {
        let accept = async {
            let (conn, addr) = lis.accept().await.context("failed to accept connection")?;
            assert!(addr.ip().is_loopback());
            anyhow::Ok(spawn(wrpc_test::serve_http(conn, |_, header, req| {
                wrpc_test::serve_http_invocation(header, req)
            })))
        };
        let (conn, ()) = try_join!(
            accept,
            wrpc_test::assert_http_invocation(Client::new(uri, &clt))
        )?;
        drop(clt);
        conn.await.context("connection task panicked")?
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn response_parts() -> anyhow::Result<()> {
    let serve = async |stream| {
        wrpc_test::serve_http(stream, |parts, _, mut req| async move {
            req.response.status = StatusCode::IM_A_TEAPOT;
            let headers = &mut req.response.headers;
            headers.insert("foo", HeaderValue::from_static("bar"));
            headers.insert("req-content-type", parts.headers[CONTENT_TYPE].clone());
            let res =
                serve::<(), _, _, _>(req, async |()| Ok((Bytes::from_static(b"baz"),))).await?;
            Ok(res)
        })
        .await
        .context("failed to serve HTTP/2 connection")
    };
    wrpc_test::with_http2(serve, |uri, mut sender| async move {
        sender.ready().await?;
        let clt = Client::new(
            uri.clone(),
            tower::util::service_fn(|req: http::Request<OutgoingParams>| sender.send_request(req)),
        );
        let ((buf,), io) = invoke::<(), (Bytes,), _>(clt, "foo", "bar", ()).await?;
        io.await?;
        assert_eq!(buf, "baz");

        sender.ready().await?;
        let clt = Client::new(
            uri,
            tower::util::service_fn(|req: http::Request<OutgoingParams>| {
                let res = sender.send_request(req);
                async {
                    let res = res.await?;
                    let headers = res.headers();
                    assert_eq!(headers[CONTENT_TYPE], wrpc_http::MEDIA_TYPE);
                    assert_eq!(headers["req-content-type"], wrpc_http::MEDIA_TYPE);
                    assert_eq!(headers["foo"], "bar");
                    ensure!(res.status().is_success(), "status `{}`", res.status());
                    Ok(res)
                }
            }),
        );
        let res = invoke::<(), (Bytes,), _>(clt, "foo", "bar", ()).await;
        let Err(InvokeError::Request(err)) = res else {
            bail!("the service should fail");
        };
        assert_eq!(err.to_string(), "status `418 I'm a teapot`");
        Ok(())
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn buffered() -> anyhow::Result<()> {
    let params = Bytes::from(vec![0x42; 65536]);
    let clt = Client::new(
        Uri::default(),
        tower::util::service_fn(|req: http::Request<OutgoingParams>| async {
            let size = req.body().size_hint().exact();
            let (parts, body) = req.into_parts();
            let body = body.collect().await?;
            let body = body.to_bytes();
            ensure!(size == u64::try_from(body.len()).ok());
            let (Header { instance, name }, srv) = wrpc_http::accept(Full::new(body)).await?;
            ensure!(instance == "foo" && name == "bar");
            let res = serve::<Bytes, _, _, _>(srv, async |params| Ok(params)).await?;
            let (res, body) = res.into_parts();
            let body = body.collect().await?;
            ensure!(parts.headers[CONTENT_TYPE] == res.headers[CONTENT_TYPE]);
            anyhow::Ok(http::Response::from_parts(res, Full::new(body.to_bytes())))
        }),
    );
    let (results, io) = invoke::<Bytes, Bytes, _>(clt, "foo", "bar", params.clone()).await?;
    io.await?;
    assert_eq!(results, params);
    Ok(())
}

#[test_log::test(tokio::test)]
async fn local() -> anyhow::Result<()> {
    let name = Rc::new(String::from("world"));
    let name = &name;
    let clt = Client::new(
        Uri::default(),
        tower::util::service_fn(move |req: http::Request<OutgoingParams>| async move {
            let (
                Header {
                    instance,
                    name: func,
                },
                srv,
            ) = wrpc_http::accept(req.into_body()).await?;
            ensure!(instance == "foo" && func == "bar");
            let res = srv
                .serve(
                    Codec::<
                        <(String,) as Encode>::Encoder,
                        <(LocalBoxFuture<'static, String>,) as Decode>::Decoder,
                    >::default(),
                    async |(greeting,)| Ok((format!("{}, {name}", greeting.await),)),
                )
                .await?;
            let (res, body) = res.into_parts();
            let body = body.collect().await?;
            anyhow::Ok(http::Response::from_parts(res, Full::new(body.to_bytes())))
        }),
    );
    let greeting: BoxFuture<'static, String> = Box::pin(async { "Hello".into() });
    let ((greeting,), io) = invoke::<_, (String,), _>(clt, "foo", "bar", (greeting,)).await?;
    io.await?;
    assert_eq!(greeting, "Hello, world");
    Ok(())
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn params_error() -> anyhow::Result<()> {
    use hyper::client::conn::http1::handshake;
    use hyper::server::conn::http1::Builder;

    async fn assert_params_error<Fut>(
        mut results: mpsc::UnboundedReceiver<anyhow::Result<()>>,
        send: impl FnMut(http::Request<OutgoingParams>) -> Fut,
    ) -> anyhow::Result<()>
    where
        Fut: Future<Output = hyper::Result<http::Response<Incoming>>>,
    {
        let (tx, rx) = oneshot::channel::<()>();
        let s: BoxFuture<'static, Utf8Bytes<&'static [u8]>> = Box::pin(async {
            _ = rx.await;
            Utf8Bytes(&b"\xff"[..])
        });
        let clt = Client::new(Uri::default(), tower::util::service_fn(send));
        let ((), io) = invoke::<_, (), _>(clt, "foo", "bar", (s,)).await?;
        _ = tx.send(());
        let err = io
            .await
            .expect_err("parameter error must fail the invocation");
        assert!(
            matches!(
                &err,
                InvokeError::Request(err)
                    if err.kind() == ErrorKind::InvalidInput
                        && err.get_ref().is_some_and(|err| err.is::<Utf8Error>())
            ),
            "{err:?}"
        );
        let res = results.next().await.context("missing invocation")?;
        res.context("a peer parameter error must not fail serving")
    }

    let f = |_, req| {
        serve::<(BoxFuture<'static, String>,), (), _, _>(req, async |(s,)| {
            spawn(s);
            Ok(())
        })
    };

    let (tx, results) = mpsc::unbounded();
    let (clt, srv) = duplex(65536);
    let (mut sender, conn) = handshake(TokioIo::new(clt)).await?;
    spawn(conn);
    spawn(Builder::new().serve_connection(TokioIo::new(srv), service(f, tx)));
    sender.ready().await?;
    assert_params_error(results, |req| sender.send_request(req)).await?;

    let (mut sender, results) = connect(f).await?;
    sender.ready().await?;
    assert_params_error(results, |req| sender.send_request(req)).await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn drop_invocation() -> anyhow::Result<()> {
    let (mut sender, mut results) = connect(|_, req| {
        serve::<(BoxStream<'static, Bytes>,), (), _, _>(req, async |(s,)| {
            spawn(s.count());
            Ok(())
        })
    })
    .await?;
    let (dropped_tx, dropped_rx) = oneshot::channel::<()>();
    let s: BoxStream<'static, Bytes> =
        Box::pin(stream::repeat(Bytes::from_static(b"foo")).then(move |buf| {
            _ = &dropped_tx;
            async {
                sleep(Duration::from_millis(10)).await;
                buf
            }
        }));
    sender.ready().await?;
    let clt = Client::new(
        Uri::default(),
        tower::util::service_fn(|req| sender.send_request(req)),
    );
    let ((), io) = invoke::<_, (), _>(clt, "foo", "bar", (s,)).await?;
    drop(io);
    let res = tokio::time::timeout(Duration::from_secs(10), dropped_rx)
        .await
        .context("request body was not stopped")?;
    res.expect_err("parameters must not be sent in full");
    results.next().await.context("missing invocation")??;
    Ok(())
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn echo() -> anyhow::Result<()> {
    let (mut sender, ..) = connect(|_, req| {
        serve::<(BoxStream<'static, Bytes>,), _, _, _>(req, async |(s,)| Ok((s,)))
    })
    .await?;
    let (tx, rx) = mpsc::unbounded();
    tx.unbounded_send(Bytes::from_static(b"foo"))?;
    sender.ready().await?;
    let clt = Client::new(
        Uri::default(),
        tower::util::service_fn(|req| sender.send_request(req)),
    );
    let ((mut s,), io) =
        invoke::<_, (BoxStream<'static, Bytes>,), _>(clt, "foo", "echo", (rx.boxed(),)).await?;
    let fut = async {
        try_join!(
            async { io.await.context("invocation failed") },
            async move {
                assert_eq!(s.next().await, Some(Bytes::from_static(b"foo")));
                tx.unbounded_send(Bytes::from_static(b"bar"))?;
                assert_eq!(s.next().await, Some(Bytes::from_static(b"bar")));
                drop(tx);
                assert_eq!(s.next().await, None);
                Ok(())
            }
        )
    };
    tokio::time::timeout(Duration::from_secs(10), fut)
        .await
        .context("response was not streamed")??;
    Ok(())
}
