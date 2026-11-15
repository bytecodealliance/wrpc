#![cfg(feature = "net")]

use core::time::Duration;

use anyhow::Context as _;
use bytes::{Bytes, BytesMut};
use futures::stream::{self, BoxStream};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::join;
use tokio::net::TcpStream;
use tokio::task::yield_now;
use tokio_util::codec::{Encoder as _, FramedRead};
use wrpc_transport::{Header, HeaderDecoder, HeaderEncoder, HeaderRef, invoke};

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn loopback() -> anyhow::Result<()> {
    wrpc_test::serve_tcp(async |addr, lis| {
        let connect = || wrpc_test::tcp_streams(addr);
        let accept = async || wrpc_test::tcp_accept(&lis).await;
        let (tx, rx) = connect().await?;
        wrpc_test::assert_single_invocation(tx, rx, accept).await?;
        wrpc_test::assert_invalid_data_resets(connect, accept).await
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn unprocessed_stream() -> anyhow::Result<()> {
    wrpc_test::serve_tcp(async |addr, lis| {
        let fut = async {
            join!(
                async {
                    let (tx, rx) = wrpc_test::tcp_streams(addr)
                        .await
                        .expect("failed to connect");
                    let s: BoxStream<'static, Bytes> = Box::pin(stream::unfold((), async |()| {
                        yield_now().await;
                        Some((Bytes::from_static(b"test"), ()))
                    }));
                    let res = invoke(tx, rx, "foo", "bar", (s,)).await;
                    if let Ok(((), io)) = res {
                        _ = io.await;
                    }
                },
                async {
                    let (_tx, rx) = wrpc_test::tcp_accept(&lis)
                        .await
                        .expect("failed to accept connection");
                    _ = wrpc_test::read_invocation(
                        FramedRead::new(rx, HeaderDecoder::default()),
                        "foo",
                        "bar",
                    )
                    .await
                    .expect("failed to read invocation");
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
async fn dropped_invocation_closes() -> anyhow::Result<()> {
    let lis = wrpc_test::tcp_bind().await?;
    let addr = lis.local_addr()?;
    let mut clt = TcpStream::connect(addr).await?;
    let mut buf = BytesMut::default();
    HeaderEncoder.encode(
        HeaderRef {
            instance: "foo",
            name: "bar",
        },
        &mut buf,
    )?;
    clt.write_all(&buf).await?;
    let (stream, ..) = lis.accept().await?;
    let (rx, tx) = stream.into_split();
    let (header, rx) = Header::read(rx).await?;
    assert_eq!(header.name, "bar");
    drop(tx);
    drop(rx);
    let n = clt.read_to_end(&mut Vec::default()).await?;
    assert_eq!(n, 0);
    Ok(())
}
