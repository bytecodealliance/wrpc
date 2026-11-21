use core::time::Duration;

use std::rc::Rc;

use anyhow::{Context as _, ensure};
use bytes::{Bytes, BytesMut};
use futures::StreamExt as _;
use futures::future::LocalBoxFuture;
use futures::stream::{self, BoxStream};
use http::header::CONTENT_TYPE;
use http::{HeaderValue, StatusCode};
use http_body::Body as _;
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;
use tokio::io::AsyncReadExt as _;
use tokio::time::sleep;
use tokio::{spawn, try_join};
use tokio_util::codec::{Encoder as _, FramedRead};
use wrpc_http::{Body, Client};
use wrpc_transport::{
    Codec, Decode, Encode, FrameDecoder, FrameEncoder, FrameRef, Header, HeaderEncoder, HeaderRef,
    invoke, invoke_blocking, receive, serve,
};

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn http2() -> anyhow::Result<()> {
    wrpc_test::with_http2(
        async |stream| {
            wrpc_http::serve(TokioIo::new(stream), |_, header, srv| async move {
                if header.name == "baz" {
                    return serve::<(), (), _, _, _>(srv, async |()| anyhow::Ok(())).await;
                }
                wrpc_test::serve_http_invocation(header, srv).await
            })
            .await
            .context("failed to serve HTTP/2 connection")
        },
        |parts, mut sender| async move {
            sender.ready().await?;
            wrpc_test::assert_http_invocation(parts.clone(), |req| {
                sender.send_request(req.map(BodyExt::boxed_unsync))
            })
            .await?;

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
                let body = Full::new(buf.freeze()).map_err(|err| match err {});
                let req = http::Request::from_parts(parts.clone(), body.boxed_unsync());
                sender.ready().await?;
                let res = wrpc_http::response(sender.send_request(req))
                    .read_to_end(&mut Vec::default())
                    .await;
                res.expect_err("invalid data must fail the response");
            }
            Ok(())
        },
    )
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn unread_params() -> anyhow::Result<()> {
    wrpc_test::with_http2(
        async |stream| {
            wrpc_http::serve(TokioIo::new(stream), |_, _, srv| {
                serve::<(BoxStream<'static, Bytes>,), (), _, _, _>(srv, async |(s,)| {
                    drop(s);
                    anyhow::Ok(())
                })
            })
            .await
            .context("failed to serve HTTP/2 connection")
        },
        |parts, mut sender| async move {
            let s: BoxStream<'static, Bytes> = Box::pin(
                stream::repeat(Bytes::from_static(b"foo")).then(async |buf| {
                    sleep(Duration::from_millis(10)).await;
                    buf
                }),
            );
            sender.ready().await?;
            let clt = Client {
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
    )
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn pooling() -> anyhow::Result<()> {
    wrpc_test::with_http_pooling(|parts, clt, lis| async move {
        let accept = async {
            let (conn, addr) = lis.accept().await.context("failed to accept connection")?;
            assert!(addr.ip().is_loopback());
            anyhow::Ok(spawn(wrpc_test::serve_http1(conn, |_, header, srv| {
                wrpc_test::serve_http_invocation(header, srv)
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
        wrpc_http::serve(TokioIo::new(stream), |req, _, srv| async move {
            let mut res =
                serve::<(), _, _, _, _>(srv, async |()| anyhow::Ok((Bytes::from_static(b"baz"),)))
                    .await?;
            *res.status_mut() = StatusCode::IM_A_TEAPOT;
            let headers = res.headers_mut();
            headers.insert("foo", HeaderValue::from_static("bar"));
            headers.insert("req-content-type", req.headers[CONTENT_TYPE].clone());
            anyhow::Ok(res)
        })
        .await
        .context("failed to serve HTTP/2 connection")
    };
    wrpc_test::with_http2(serve, |parts, mut sender| async move {
        let req = wrpc_http::request(
            parts.clone(),
            <() as Encode>::Encoder::default(),
            "foo",
            "bar",
            (),
        )?;
        sender.ready().await?;
        let rx = wrpc_http::response(sender.send_request(req));
        let rx = FramedRead::new(rx, FrameDecoder::default());
        let ((buf,), io) = receive(<(Bytes,) as Decode>::Decoder::default(), rx).await?;
        io.await?;
        assert_eq!(buf, "baz");

        let req = wrpc_http::request(parts, <() as Encode>::Encoder::default(), "foo", "bar", ())?;
        sender.ready().await?;
        let res = sender.send_request(req);
        let rx = wrpc_http::response(async {
            let res = res.await?;
            let headers = res.headers();
            assert_eq!(headers[CONTENT_TYPE], wrpc_http::MEDIA_TYPE);
            assert_eq!(headers["req-content-type"], wrpc_http::MEDIA_TYPE);
            assert_eq!(headers["foo"], "bar");
            ensure!(res.status().is_success(), "status `{}`", res.status());
            Ok(res)
        });
        let rx = FramedRead::new(rx, FrameDecoder::default());
        let res = receive(<(Bytes,) as Decode>::Decoder::default(), rx).await;
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
    let req = wrpc_http::request(
        parts,
        <Bytes as Encode>::Encoder::default(),
        "foo",
        "bar",
        params.clone(),
    )?;
    let size = req.body().size_hint().exact();
    let rx = wrpc_http::response(async {
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
        Ok::<_, anyhow::Error>(http::Response::from_parts(res, Full::new(body.to_bytes())))
    });
    let rx = FramedRead::new(rx, FrameDecoder::default());
    let (results, io) = receive(<Bytes as Decode>::Decoder::default(), rx).await?;
    io.await?;
    assert_eq!(results, params);
    Ok(())
}

#[test_log::test(tokio::test)]
async fn local() -> anyhow::Result<()> {
    let name = Rc::new(String::from("world"));
    let (parts, ()) = http::Request::new(()).into_parts();
    let clt = Client {
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
            srv.serve(
                Codec::<
                    <(String,) as Encode>::Encoder,
                    <(LocalBoxFuture<'static, String>,) as Decode>::Decoder,
                >::default(),
                async |(greeting,)| anyhow::Ok((format!("{}, {name}", greeting.await),)),
            )
            .await
        },
    };
    let greeting: LocalBoxFuture<'static, String> = Box::pin(async { "Hello".into() });
    let (greeting,) = invoke_blocking::<_, (String,), _>(clt, "foo", "bar", (greeting,)).await?;
    assert_eq!(greeting, "Hello, world");
    Ok(())
}
