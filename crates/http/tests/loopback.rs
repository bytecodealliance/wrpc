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
use http::{HeaderValue, StatusCode};
use http_body::Body as _;
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::client::conn::http2::{SendRequest, handshake};
use hyper::server::conn::http2::Builder;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::io::duplex;
use tokio::time::sleep;
use tokio::{join, select, spawn, try_join};
use tokio_util::codec::Encoder as _;
use wrpc_core::{
    Codec, Decode, Encode, FrameEncoder, FrameRef, Header, HeaderEncoder, HeaderRef, ServeError,
    Utf8Bytes, invoke, serve,
};
use wrpc_http::{Body, Listener, OutgoingRequest};

async fn connect() -> anyhow::Result<(
    SendRequest<UnsyncBoxBody<Bytes, std::io::Error>>,
    Listener<'static>,
)> {
    let (clt, srv) = duplex(65536);
    let (sender, conn) = handshake(TokioExecutor::default(), TokioIo::new(clt)).await?;
    spawn(conn);
    let lis = Listener::http2(&Builder::new(TokioExecutor::default()), TokioIo::new(srv));
    Ok((sender, lis))
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
        let (mut sender, mut lis) = connect().await?;
        try_join!(
            async {
                let (_, Header { name, .. }, req) =
                    lis.next().await.context("missing invocation")??;
                ensure!(name == "baz", "unexpected `{name}`");
                let (res, next) = join!(
                    serve::<(), (), _, _, _>(req, async |()| anyhow::Ok(())),
                    lis.next()
                );
                match res {
                    Err(ServeError::Request(..)) => {}
                    Ok(()) if path.is_empty() => {}
                    res => bail!("unexpected result `{res:?}`"),
                }
                ensure!(next.is_none(), "connection must close cleanly");
                Ok(())
            },
            async move {
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
                Ok(())
            },
        )?;
    }
    Ok(())
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn unread_params() -> anyhow::Result<()> {
    let (mut sender, mut lis) = connect().await?;
    try_join!(
        async {
            let (_, _, req) = lis.next().await.context("missing invocation")??;
            let (res, next) = join!(
                serve::<(BoxStream<'static, Bytes>,), (), _, _, _>(req, async |(s,)| {
                    drop(s);
                    anyhow::Ok(())
                }),
                lis.next()
            );
            res?;
            ensure!(next.is_none(), "connection must close cleanly");
            Ok(())
        },
        async move {
            let s: BoxStream<'static, Bytes> = Box::pin(
                stream::repeat(Bytes::from_static(b"foo")).then(async |buf| {
                    sleep(Duration::from_millis(10)).await;
                    buf
                }),
            );
            sender.ready().await?;
            let (parts, ()) = http::Request::new(()).into_parts();
            let clt = OutgoingRequest {
                parts,
                send: |req: http::Request<Body<_>>| {
                    sender.send_request(req.map(BodyExt::boxed_unsync))
                },
            };
            let res = async {
                let ((), io) = invoke::<_, (), _>(clt, "foo", "bar", (s,)).await?;
                io.await
            };
            res.await
                .expect_err("unread parameters must fail the invocation");
            Ok(())
        },
    )?;
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
    wrpc_test::with_http2(
        async |stream| {
            wrpc_test::serve_http(stream, move |_, header, req| {
                let done = Done(done_tx.clone());
                let started_tx = started_tx.clone();
                async move {
                    if header.name != "stream" {
                        return wrpc_test::serve_http_invocation(header, req).await;
                    }
                    let _done = done;
                    serve::<(BoxStream<'static, Bytes>,), (), _, _, _>(req, async |(_s,)| {
                        started_tx.unbounded_send(())?;
                        future::pending::<anyhow::Result<()>>().await
                    })
                    .await?;
                    Ok(())
                }
            })
            .await
            .context("a peer cancel must not fail the connection")
        },
        |parts, mut sender| async move {
            let s: BoxStream<'static, Bytes> = Box::pin(
                stream::repeat(Bytes::from_static(b"foo")).then(async |buf| {
                    sleep(Duration::from_millis(10)).await;
                    buf
                }),
            );
            sender.ready().await?;
            let clt = OutgoingRequest {
                parts: parts.clone(),
                send: |req: http::Request<Body<_>>| {
                    sender.send_request(req.map(BodyExt::boxed_unsync))
                },
            };
            select! {
                _ = invoke::<_, (), _>(clt, "foo", "stream", (s,)) => {
                    bail!("invocation must not complete");
                }
                _ = started_rx.next() => {}
            }
            done_rx.next().await.context("invocation was not served")?;
            sender.ready().await?;
            wrpc_test::assert_http_invocation(parts, |req| sender.send_request(req)).await
        },
    )
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn pooling() -> anyhow::Result<()> {
    wrpc_test::with_http_pooling(|parts, clt, lis| async move {
        let accept = async {
            let (conn, addr) = lis.accept().await.context("failed to accept connection")?;
            assert!(addr.ip().is_loopback());
            anyhow::Ok(spawn(wrpc_test::serve_http(conn, |_, header, req| {
                wrpc_test::serve_http_invocation(header, req)
            })))
        };
        let (conn, ()) = try_join!(
            accept,
            wrpc_test::assert_http_invocation(parts, |req| clt.request(req))
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
            req.req.parts.status = StatusCode::IM_A_TEAPOT;
            let headers = &mut req.req.parts.headers;
            headers.insert("foo", HeaderValue::from_static("bar"));
            headers.insert("req-content-type", parts.headers[CONTENT_TYPE].clone());
            serve::<(), _, _, _, _>(req, async |()| anyhow::Ok((Bytes::from_static(b"baz"),)))
                .await?;
            Ok(())
        })
        .await
        .context("failed to serve HTTP/2 connection")
    };
    wrpc_test::with_http2(serve, |parts, mut sender| async move {
        sender.ready().await?;
        let clt = OutgoingRequest {
            parts: parts.clone(),
            send: |req: http::Request<Body<_>>| sender.send_request(req),
        };
        let ((buf,), io) = invoke::<(), (Bytes,), _>(clt, "foo", "bar", ()).await?;
        io.await?;
        assert_eq!(buf, "baz");

        sender.ready().await?;
        let clt = OutgoingRequest {
            parts,
            send: |req: http::Request<Body<_>>| {
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
            },
        };
        let res = invoke::<(), (Bytes,), _>(clt, "foo", "bar", ()).await;
        let err = res.err().context("receiving results should fail")?;
        assert_eq!(err.to_string(), "status `418 I'm a teapot`");
        Ok(())
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn buffered() -> anyhow::Result<()> {
    let params = Bytes::from(vec![0x42; 65536]);
    let (parts, ()) = http::Request::new(()).into_parts();
    let clt = OutgoingRequest {
        parts,
        send: |req: http::Request<Body<_>>| async {
            let size = req.body().size_hint().exact();
            let (parts, body) = req.into_parts();
            let body = body.collect().await?;
            let body = body.to_bytes();
            ensure!(size == u64::try_from(body.len()).ok());
            let (Header { instance, name }, srv) = wrpc_http::accept(Full::new(body)).await?;
            ensure!(instance == "foo" && name == "bar");
            let res = serve::<Bytes, _, _, _, _>(srv, async |params| anyhow::Ok(params)).await?;
            let (res, body) = res.into_parts();
            let body = body.collect().await?;
            ensure!(parts.headers[CONTENT_TYPE] == res.headers[CONTENT_TYPE]);
            Ok(http::Response::from_parts(res, Full::new(body.to_bytes())))
        },
    };
    let (results, io) = invoke::<Bytes, Bytes, _>(clt, "foo", "bar", params.clone()).await?;
    io.await?;
    assert_eq!(results, params);
    Ok(())
}

#[test_log::test(tokio::test)]
async fn local() -> anyhow::Result<()> {
    let name = Rc::new(String::from("world"));
    let (parts, ()) = http::Request::new(()).into_parts();
    let clt = OutgoingRequest {
        parts,
        send: |req: http::Request<Body<_>>| async move {
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
                    async |(greeting,)| anyhow::Ok((format!("{}, {name}", greeting.await),)),
                )
                .await?;
            let (res, body) = res.into_parts();
            let body = body.collect().await?;
            Ok(http::Response::from_parts(res, Full::new(body.to_bytes())))
        },
    };
    let greeting: LocalBoxFuture<'static, String> = Box::pin(async { "Hello".into() });
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
        mut lis: Listener<'static>,
        send: impl FnOnce(http::Request<UnsyncBoxBody<Bytes, std::io::Error>>) -> Fut,
    ) -> anyhow::Result<()>
    where
        Fut: Future<Output = hyper::Result<http::Response<Incoming>>>,
    {
        try_join!(
            async {
                let (_, _, req) = lis.next().await.context("missing invocation")??;
                let (res, _) = join!(
                    serve::<(BoxFuture<'static, String>,), (), _, _, _>(req, async |(s,)| {
                        spawn(s);
                        anyhow::Ok(())
                    }),
                    lis.next()
                );
                res.context("a peer parameter error must not fail serving")
            },
            async move {
                let (tx, rx) = oneshot::channel::<()>();
                let s: BoxFuture<'static, Utf8Bytes<&'static [u8]>> = Box::pin(async {
                    _ = rx.await;
                    Utf8Bytes(&b"\xff"[..])
                });
                let (parts, ()) = http::Request::new(()).into_parts();
                let clt = OutgoingRequest {
                    parts,
                    send: |req: http::Request<Body<_>>| send(req.map(BodyExt::boxed_unsync)),
                };
                let ((), io) = invoke::<_, (), _>(clt, "foo", "bar", (s,)).await?;
                _ = tx.send(());
                let err = io
                    .await
                    .expect_err("parameter error must fail the invocation");
                assert_eq!(err.kind(), ErrorKind::InvalidData);
                assert!(err.get_ref().is_some_and(|err| err.is::<Utf8Error>()));
                Ok(())
            },
        )?;
        Ok(())
    }

    let (clt, srv) = duplex(65536);
    let (mut sender, conn) = handshake(TokioIo::new(clt)).await?;
    spawn(conn);
    let lis = Listener::http1(&Builder::new(), TokioIo::new(srv));
    sender.ready().await?;
    assert_params_error(lis, move |req| sender.send_request(req)).await?;

    let (mut sender, lis) = connect().await?;
    sender.ready().await?;
    assert_params_error(lis, move |req| sender.send_request(req)).await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn drop_invocation() -> anyhow::Result<()> {
    let (mut sender, mut lis) = connect().await?;
    try_join!(
        async {
            let (_, _, req) = lis.next().await.context("missing invocation")??;
            let (res, next) = join!(
                serve::<(BoxStream<'static, Bytes>,), (), _, _, _>(req, async |(s,)| {
                    spawn(s.count());
                    anyhow::Ok(())
                }),
                lis.next()
            );
            res?;
            ensure!(next.is_none(), "connection must close cleanly");
            Ok(())
        },
        async move {
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
            let (parts, ()) = http::Request::new(()).into_parts();
            let clt = OutgoingRequest {
                parts,
                send: |req: http::Request<Body<_>>| {
                    sender.send_request(req.map(BodyExt::boxed_unsync))
                },
            };
            let ((), io) = invoke::<_, (), _>(clt, "foo", "bar", (s,)).await?;
            drop(io);
            let res = tokio::time::timeout(Duration::from_secs(10), dropped_rx)
                .await
                .context("request body was not stopped")?;
            res.expect_err("parameters must not be sent in full");
            Ok(())
        },
    )?;
    Ok(())
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn service_fn() -> anyhow::Result<()> {
    let (clt, srv) = duplex(65536);
    let (mut sender, conn) = handshake(TokioExecutor::default(), TokioIo::new(clt)).await?;
    spawn(conn);
    let svc = hyper::service::service_fn(|req: http::Request<Incoming>| async move {
        let (Header { instance, name }, req) = wrpc_http::accept(req.into_body()).await?;
        ensure!(instance == "foo" && name == "echo");
        let res =
            serve::<(BoxStream<'static, Bytes>,), _, _, _, _>(req, async |(s,)| anyhow::Ok((s,)))
                .await?;
        anyhow::Ok(res)
    });
    spawn(Builder::new(TokioExecutor::default()).serve_connection(TokioIo::new(srv), svc));

    let (tx, rx) = mpsc::unbounded();
    tx.unbounded_send(Bytes::from_static(b"foo"))?;
    sender.ready().await?;
    let (parts, ()) = http::Request::new(()).into_parts();
    let clt = OutgoingRequest {
        parts,
        send: |req: http::Request<Body<_>>| sender.send_request(req.map(BodyExt::boxed_unsync)),
    };
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
