use core::time::Duration;

use std::sync::Arc;

use anyhow::{Context as _, anyhow};
use bytes::Bytes;
use http_body_util::{BodyExt as _, Full};
use hyper::service::service_fn;
use hyper_util::rt::TokioExecutor;
use hyper_util::rt::tokio::WithHyperIo;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::{spawn, try_join};
use wrpc_http::{Client, OutgoingBody, Server};
use wrpc_transport::Invoke as _;

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn http2() -> anyhow::Result<()> {
    wrpc_test::serve_http2(async |parts, clt, srv| {
        wrpc_test::assert_single_invocation(parts, &clt, srv).await?;
        Ok(())
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn pooling() -> anyhow::Result<()> {
    wrpc_test::with_http_pooling(|parts, clt, lis| async move {
        let srv = Arc::new(wrpc_transport::frame::Server::default());
        let clt = Client::from(clt);
        let accept = async {
            let (conn, addr) = lis.accept().await.context("failed to accept connection")?;
            assert!(addr.ip().is_loopback());
            let conn = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                .serve_connection(WithHyperIo::new(conn), Server::from(Arc::clone(&srv)))
                .into_owned();
            anyhow::Ok(spawn(conn))
        };
        let (conn, _) = try_join!(
            accept,
            wrpc_test::assert_single_invocation(parts, &clt, srv.as_ref())
        )?;
        drop(clt);
        conn.await
            .context("connection task panicked")?
            .map_err(|err| anyhow!(err).context("failed to serve connection"))?;
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
    let fut = wrpc_test::with_http2(svc, |parts, sender| async move {
        let (tx, mut rx) = Client::from(sender)
            .invoke(parts, "foo", "bar", [[None; 0]; 0])
            .await?;
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
    for shutdown in [true, false] {
        let (body, mut tx) = OutgoingBody::new([]);
        tx.write_all(b"foo").await.context("failed to write body")?;
        if shutdown {
            tx.shutdown().await.context("failed to shut down body")?;
        }
        drop(tx);
        let res = body.collect().await;
        if shutdown {
            assert_eq!(res.context("failed to read body")?.to_bytes(), "foo");
        } else {
            assert_eq!(res.unwrap_err().kind(), std::io::ErrorKind::BrokenPipe);
        }
    }
    Ok(())
}
