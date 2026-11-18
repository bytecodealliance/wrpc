use core::time::Duration;

use anyhow::{Context as _, ensure};
use bytes::Bytes;
use http::{HeaderValue, StatusCode};
use http_body_util::{BodyExt as _, Empty, Full};
use hyper::server::conn::http2;
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::mpsc;
use tokio::{spawn, try_join};
use tokio_util::codec::{FramedRead, FramedWrite};
use wrpc_http::OutgoingBody;
use wrpc_transport::{FrameDecoder, FrameEncoder, Listener};

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn http2() -> anyhow::Result<()> {
    wrpc_test::serve_http2(async |parts, sender, srv| {
        let open = || wrpc_http::open(sender.clone(), parts.clone());
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
        wrpc_http::serve(TokioIo::new(stream), |_, res, tx, mut rx| {
            res.status = StatusCode::ACCEPTED;
            res.headers.insert("foo", HeaderValue::from_static("bar"));
            drop(tx);
            spawn(async move { rx.read_to_end(&mut Vec::default()).await });
        })
        .await
        .context("failed to serve connection")
    };
    wrpc_test::with_http2(serve, |parts, mut sender| async move {
        let req = http::Request::from_parts(parts, Empty::<Bytes>::default());
        let res = sender.send_request(req).await?;
        assert_eq!(res.status(), StatusCode::ACCEPTED);
        assert_eq!(res.headers()["foo"], "bar");
        Ok(())
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn error_response() -> anyhow::Result<()> {
    let svc = service_fn(|req: hyper::Request<hyper::body::Incoming>| async {
        req.into_body().collect().await?;
        let mut res = hyper::Response::new(Full::new(Bytes::from("nope")));
        *res.status_mut() = hyper::StatusCode::INTERNAL_SERVER_ERROR;
        anyhow::Ok(res)
    });
    let serve = async |stream| {
        http2::Builder::new(TokioExecutor::default())
            .serve_connection(TokioIo::new(stream), svc)
            .await
            .context("failed to serve connection")
    };
    let fut = wrpc_test::with_http2(serve, |parts, sender| async move {
        let (mut tx, mut rx) = wrpc_http::open(sender, parts).await?;
        tx.shutdown().await?;
        drop(tx);
        let err = rx
            .read_to_end(&mut vec![])
            .await
            .expect_err("reading results should fail");
        assert_eq!(
            err.to_string(),
            "HTTP request failed with status `500 Internal Server Error`: nope"
        );
        Ok(())
    });
    tokio::time::timeout(Duration::from_secs(10), fut).await?
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
