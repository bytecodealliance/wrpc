#![cfg(feature = "net")]

use core::pin::pin;
use core::time::Duration;

use std::sync::Arc;

use anyhow::Context as _;
use futures::TryStreamExt as _;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::join;
use tokio::net::TcpStream;
use wrpc_transport::tcp;
use wrpc_transport::{Invoke as _, Serve as _};

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
                        .invoke((), "foo", "bar", [[Some(0)]])
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
    for shutdown in [true, false] {
        let (clt, srv) = join!(TcpStream::connect(addr), lis.accept());
        let clt = clt?;
        let (mut rx, mut tx) = tcp::split(clt);
        tx.write_all(b"foo").await?;
        if shutdown {
            tx.shutdown().await?;
        }
        drop(tx);
        let (mut srv, ..) = srv?;
        let mut buf = Vec::default();
        let res = srv.read_to_end(&mut buf).await;
        if shutdown {
            res?;
            assert_eq!(buf, b"foo");
        } else {
            assert_eq!(res.unwrap_err().kind(), std::io::ErrorKind::ConnectionReset);
            let err = rx.read_to_end(&mut Vec::default()).await.unwrap_err();
            assert_eq!(err.kind(), std::io::ErrorKind::ConnectionAborted);
        }
    }
    Ok(())
}
