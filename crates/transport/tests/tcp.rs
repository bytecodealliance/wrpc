#![cfg(feature = "net")]

use core::time::Duration;

use anyhow::Context as _;
use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::join;
use tokio::net::TcpStream;
use tokio_util::codec::{BytesCodec, Encoder as _};
use wrpc_transport::{Connect as _, HeaderEncoder, HeaderRef, accept, invoke, tcp};

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn loopback() -> anyhow::Result<()> {
    wrpc_test::serve_tcp(async |clt, srv| {
        wrpc_test::assert_single_invocation((), &clt, tcp::incoming(srv)).await?;
        wrpc_test::assert_invalid_data_resets((), &clt, tcp::incoming(srv)).await
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn unprocessed_stream() -> anyhow::Result<()> {
    wrpc_test::serve_tcp(async |clt, srv| {
        let fut = async {
            join!(
                async {
                    let (tx, rx) = clt.connect(()).await.expect("failed to connect");
                    let (tx, _rx) = invoke(tx, rx, "foo", "bar", [[Some(0)]])
                        .expect("failed to invoke `foo.bar`");
                    let mut tx = tx.index(&[0]).expect("failed to index stream");
                    while let Ok(()) = tx
                        .encode(&mut BytesCodec::default(), Bytes::from_static(b"test"))
                        .await
                    {}
                },
                async {
                    _ = wrpc_test::accept_framed(
                        &mut tcp::incoming(srv),
                        "foo",
                        "bar",
                        [[Some(0)]],
                    )
                    .await
                    .expect("failed to accept invocation");
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
    let (mut rx, tx) = stream.into_split();
    let header = accept(&mut rx).await?;
    assert_eq!(header.name, "bar");
    drop(tx);
    drop(rx);
    let n = clt.read_to_end(&mut Vec::default()).await?;
    assert_eq!(n, 0);
    Ok(())
}
