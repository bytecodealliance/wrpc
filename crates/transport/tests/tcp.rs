#![cfg(feature = "net")]

use core::future::pending;
use core::pin::pin;
use core::time::Duration;

use std::sync::Arc;

use anyhow::Context as _;
use bytes::Bytes;
use futures::TryStreamExt as _;
use futures::future::BoxFuture;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio::{join, try_join};
use wrpc_transport::tcp;
use wrpc_transport::{Invoke as _, InvokeExt as _, Serve as _, ServeExt as _, Server};

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn loopback() -> anyhow::Result<()> {
    wrpc_test::serve_tcp(async |clt, srv| wrpc_test::assert_single_invocation((), &clt, srv).await)
        .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn unprocessed_stream() -> anyhow::Result<()> {
    wrpc_test::serve_tcp(async |clt, srv| {
        let invocations = srv
            .serve("foo", "bar", Arc::from([Box::from([Some(0)])]))
            .await?;
        let fut = async {
            join!(
                async {
                    let (tx, _rx) = clt
                        .invoke((), "foo", "bar", Bytes::default(), [[Some(0)]])
                        .await
                        .expect("failed to invoke `foo.bar`");
                    let mut tx = tx.index(&[0]).expect("failed to index stream");
                    while let Ok(()) = tx.write_all(b"test").await {}
                },
                async {
                    _ = pin!(invocations)
                        .try_next()
                        .await
                        .expect("failed to accept invocation")
                        .expect("invocation missing");
                }
            )
        };
        tokio::time::timeout(Duration::from_secs(10), fut)
            .await
            .context("unprocessed stream data did not fail the invocation")?;
        Ok(())
    })
    .await
}

#[test_log::test(tokio::test)]
async fn write_half_dropped() -> anyhow::Result<()> {
    let lis = wrpc_test::tcp_bind().await?;
    let addr = lis.local_addr()?;
    let (clt, srv) = join!(TcpStream::connect(addr), lis.accept());
    let clt = clt?;
    let (mut rx, mut tx) = tcp::split(clt);
    tx.write_all(b"foo").await?;
    drop(tx);
    let (mut srv, ..) = srv?;
    let mut buf = Vec::default();
    srv.read_to_end(&mut buf).await?;
    assert_eq!(buf, b"foo");
    srv.write_all(b"bar").await?;
    drop(srv);
    buf.clear();
    rx.read_to_end(&mut buf).await?;
    assert_eq!(buf, b"bar");
    Ok(())
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn undecodable_values() -> anyhow::Result<()> {
    wrpc_test::serve_tcp(async |clt, srv| {
        let params = srv
            .serve_values::<(String,), ()>("foo", "params", Arc::default())
            .await?;
        let results = srv
            .serve("foo", "results", Arc::from([Box::from([Some(0)])]))
            .await?;
        let (mut params, mut results) = (pin!(params), pin!(results));
        try_join!(
            async {
                let res = params.try_next().await;
                assert!(res.is_err(), "decoding invalid parameters should fail");
                let invocation = results.try_next().await?;
                let ((), mut tx, rx) = invocation.context("invocation missing")?;
                tx.write_all(b"\x01\xff").await?;
                let res = timeout(Duration::from_secs(5), rx.finished()).await?;
                res.expect_err("invalid results should have been rejected");
                anyhow::Ok(())
            },
            async {
                let (_tx, mut rx) = clt
                    .invoke(
                        (),
                        "foo",
                        "params",
                        Bytes::from_static(b"\x01\xff"),
                        [[]; 0],
                    )
                    .await?;
                let res = rx.read_to_end(&mut Vec::default()).await;
                res.expect_err("invalid parameters should have been rejected");
                let params: (BoxFuture<'static, String>,) = (Box::pin(pending()),);
                let res = clt
                    .invoke_values::<_, _, (String,), _>((), "foo", "results", params, [[Some(0)]])
                    .await;
                assert!(res.is_err(), "decoding invalid results should fail");
                anyhow::Ok(())
            },
        )?;
        Ok(())
    })
    .await
}

#[test_log::test(tokio::test)]
async fn unhandled_invocation() -> anyhow::Result<()> {
    let lis = wrpc_test::tcp_bind().await?;
    let clt = tcp::Client::from(lis.local_addr()?);
    for queued in [false, true] {
        let srv = Server::default();
        let mut invocations = Some(srv.serve("foo", "bar", Arc::default()).await?);
        if !queued {
            invocations.take();
        }
        let (res, conn) = join!(
            clt.invoke((), "foo", "bar", Bytes::default(), [[]; 0]),
            lis.accept()
        );
        let (_tx, mut rx) = res?;
        let (stream, ..) = conn?;
        let (srx, stx) = tcp::split(stream);
        let res = srv.accept((), stx, BufReader::new(srx)).await;
        assert_eq!(res.is_ok(), queued);
        drop(invocations);
        let res = rx.read_to_end(&mut Vec::default()).await;
        res.expect_err("unhandled invocation should have been rejected");
    }
    Ok(())
}
