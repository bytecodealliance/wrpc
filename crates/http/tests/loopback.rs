use anyhow::{Context as _, ensure};
use bytes::Bytes;
use http::header::CONTENT_TYPE;
use http::{HeaderValue, StatusCode};
use http_body_util::{BodyExt as _, Full};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::mpsc;
use tokio::{spawn, try_join};
use tokio_util::codec::{FramedRead, FramedWrite};
use wrpc_http::OutgoingBody;
use wrpc_transport::{FrameDecoder, FrameEncoder, Listener};

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn http2() -> anyhow::Result<()> {
    wrpc_test::serve_http2(async |parts, sender, srv| {
        let open = || wrpc_test::http_open(sender.clone(), parts.clone());
        let (tx, rx) = open().await?;
        wrpc_test::assert_single_invocation(tx, rx, wrpc_test::http_accept(&mut *srv)).await?;
        wrpc_test::assert_invalid_data_resets(open, async || {
            wrpc_test::http_accept(&mut *srv).await
        })
        .await
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn pooling() -> anyhow::Result<()> {
    wrpc_test::with_http_pooling(|parts, clt, lis| async move {
        let (tx, mut rx) = mpsc::channel(16);
        let accept = async {
            let (conn, addr) = lis.accept().await.context("failed to accept connection")?;
            assert!(addr.ip().is_loopback());
            anyhow::Ok(spawn(wrpc_test::serve_http1(conn, tx)))
        };
        let (req, req_tx) = wrpc_http::request(parts);
        let req_rx = wrpc_http::response(clt.request(req));
        let (conn, _) = try_join!(
            accept,
            wrpc_test::assert_single_invocation(req_tx, req_rx, wrpc_test::http_accept(&mut rx))
        )?;
        drop(clt);
        conn.await.context("connection task panicked")?
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn response_parts() -> anyhow::Result<()> {
    let serve = async |stream| {
        wrpc_http::serve(TokioIo::new(stream), |req, res, mut tx, mut rx| {
            res.status = StatusCode::IM_A_TEAPOT;
            res.headers.insert("foo", HeaderValue::from_static("bar"));
            res.headers
                .insert("req-content-type", req.headers[CONTENT_TYPE].clone());
            spawn(async move {
                rx.read_to_end(&mut Vec::default()).await?;
                tx.write_all(b"baz").await
            });
        })
        .await
        .context("failed to serve connection")
    };
    wrpc_test::with_http2(serve, |parts, sender| async move {
        let (tx, res) = wrpc_http::open(sender.clone(), parts.clone()).await?;
        drop(tx);
        let mut buf = Vec::default();
        wrpc_http::response(res).read_to_end(&mut buf).await?;
        assert_eq!(buf, b"baz");

        let (tx, res) = wrpc_http::open(sender, parts).await?;
        drop(tx);
        let err = wrpc_http::response(async {
            let res = res.await?;
            let headers = res.headers();
            assert_eq!(headers[CONTENT_TYPE], wrpc_http::MEDIA_TYPE);
            assert_eq!(headers["req-content-type"], wrpc_http::MEDIA_TYPE);
            assert_eq!(headers["foo"], "bar");
            ensure!(res.status().is_success(), "status `{}`", res.status());
            Ok(res)
        })
        .read_to_end(&mut Vec::default())
        .await
        .expect_err("reading results should fail");
        assert_eq!(err.to_string(), "status `418 I'm a teapot`");
        Ok(())
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn body_writer_dropped() -> anyhow::Result<()> {
    let (body, mut tx) = OutgoingBody::new();
    tx.write_all(b"foo").await.context("failed to write body")?;
    drop(tx);
    let res = body.collect().await.context("failed to read body")?;
    assert_eq!(res.to_bytes(), "foo");
    Ok(())
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn buffered() -> anyhow::Result<()> {
    struct Transport;

    impl Transport {
        async fn request(
            &self,
            req: http::Request<Full<Bytes>>,
        ) -> anyhow::Result<(http::response::Parts, Bytes)> {
            let (res, tx, rx) = wrpc_http::accept(req.into_body());
            let (parts, body) = res.into_parts();
            let ((), body) = try_join!(
                async {
                    let mut lis = Listener::new(tx, rx);
                    let (tx, rx) = wrpc_test::next_invocation(&mut lis, "foo", "bar").await?;
                    wrpc_transport::serve::<Bytes, Bytes, _, _, _, _>(tx, rx, async |params| {
                        anyhow::Ok(params)
                    })
                    .await?;
                    let next = lis.accept().await?;
                    ensure!(next.is_none(), "unexpected invocation");
                    Ok(())
                },
                async {
                    let body = body.collect().await?;
                    anyhow::Ok(body)
                },
            )?;
            Ok((parts, body.to_bytes()))
        }
    }

    let transport = Transport;
    let params = Bytes::from(vec![0x42; 65536]);
    let (parts, ()) = http::Request::new(()).into_parts();
    let (req, tx) = wrpc_http::request(parts);
    let rx = wrpc_http::response(async {
        let (parts, body) = req.into_parts();
        let body = body.collect().await?;
        let req = http::Request::from_parts(parts, Full::new(body.to_bytes()));
        let (parts, body) = transport.request(req).await?;
        Ok::<_, anyhow::Error>(http::Response::from_parts(parts, Full::new(body)))
    });
    let tx = FramedWrite::new(tx, FrameEncoder);
    let rx = FramedRead::new(rx, FrameDecoder::default());
    let results =
        wrpc_transport::invoke_blocking::<_, Bytes, _, _>(tx, rx, "foo", "bar", params.clone())
            .await?;
    assert_eq!(results, params);
    Ok(())
}
