#![cfg(feature = "net")]

use core::pin::pin;
use core::time::Duration;

use std::sync::Arc;

use anyhow::Context as _;
use bytes::{Bytes, BytesMut};
use futures::TryStreamExt as _;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::join;
use tokio::net::TcpStream;
use wrpc_transport::frame::encode_invocation;
use wrpc_transport::{InvokeExt as _, ServeExt as _};

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn loopback() -> anyhow::Result<()> {
    wrpc_test::serve_tcp(async |clt, srv| {
        wrpc_test::assert_single_invocation((), &clt, srv).await?;
        wrpc_test::assert_invalid_data_resets((), &clt, srv).await
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn unprocessed_stream() -> anyhow::Result<()> {
    wrpc_test::serve_tcp(async |clt, srv| {
        let invocations = srv
            .serve_framed("foo", "bar", Arc::from([Box::from([Some(0)])]))
            .await?;
        let fut = async {
            join!(
                async {
                    let (tx, _rx) = clt
                        .invoke_framed((), "foo", "bar", Bytes::default(), [[Some(0)]])
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
async fn unhandled_function_closes() -> anyhow::Result<()> {
    let lis = wrpc_test::tcp_bind().await?;
    let addr = lis.local_addr()?;
    let srv = wrpc_transport::tcp::Server::default();
    let mut clt = TcpStream::connect(addr).await?;
    let mut buf = BytesMut::default();
    encode_invocation(&mut buf, "foo", "bar", 0)?;
    clt.write_all(&buf).await?;
    wrpc_transport::tcp::accept(&srv, &lis)
        .await
        .expect_err("function must not be handled");
    let n = clt.read_to_end(&mut Vec::default()).await?;
    assert_eq!(n, 0);
    Ok(())
}
