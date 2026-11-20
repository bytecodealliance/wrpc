#![cfg(feature = "net")]

use core::time::Duration;

use anyhow::Context as _;
use bytes::{Bytes, BytesMut};
use futures::stream::{self, BoxStream};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::join;
use tokio::net::TcpStream;
use tokio::task::yield_now;
use tokio::time::timeout;
use tokio_util::codec::Encoder as _;
use wrpc_transport::{
    FrameDecoder, Framed, Header, HeaderDecoder, HeaderEncoder, HeaderRef, Listener, invoke,
};

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn loopback() -> anyhow::Result<()> {
    wrpc_test::serve_tcp(async |addr, lis| {
        let connect = || wrpc_test::tcp_streams(addr);
        let accept = async || wrpc_test::tcp_accept(&lis).await;
        let (tx, rx) = connect().await?;
        wrpc_test::assert_single_invocation(tx, rx, accept()).await?;
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
                    let res = invoke(Framed::new(tx, rx), "foo", "bar", (s,)).await;
                    if let Ok(((), io)) = res {
                        _ = io.await;
                    }
                },
                async {
                    let (tx, rx) = wrpc_test::tcp_accept(&lis)
                        .await
                        .expect("failed to accept connection");
                    let mut lis = Listener::new(tx, rx);
                    _ = wrpc_test::next_invocation(&mut lis, "foo", "bar")
                        .await
                        .expect("failed to read invocation");
                }
            )
        };
        timeout(Duration::from_secs(10), fut)
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
    let mut lis = Listener::new(tx, rx);
    let next = lis.accept().await?;
    let (header, ..) = next.context("connection closed")?;
    assert_eq!(header.name, "bar");
    drop(lis);
    let n = clt.read_to_end(&mut Vec::default()).await?;
    assert_eq!(n, 0);
    Ok(())
}

#[test_log::test(tokio::test)]
async fn conn_idle_timeout() -> anyhow::Result<()> {
    let lis = wrpc_test::tcp_bind().await?;
    let addr = lis.local_addr()?;
    let mut clt = TcpStream::connect(addr).await?;
    let (stream, ..) = lis.accept().await?;
    let (rx, tx) = stream.into_split();
    let mut lis = Listener::new(tx, rx);
    let mut buf = BytesMut::default();
    HeaderEncoder.encode(
        HeaderRef {
            instance: "foo",
            name: "bar",
        },
        &mut buf,
    )?;
    clt.write_all(&buf[..3]).await?;
    let res = timeout(Duration::from_millis(10), lis.accept()).await;
    assert!(res.is_err(), "header returned before it was sent");
    clt.write_all(&buf[3..]).await?;
    let next = lis.accept().await?;
    let (header, ..) = next.context("connection closed")?;
    assert_eq!(
        header,
        Header {
            instance: "foo".into(),
            name: "bar".into(),
        }
    );
    Ok(())
}

#[test_log::test(tokio::test)]
async fn conn_header_limits() -> anyhow::Result<()> {
    let lis = wrpc_test::tcp_bind().await?;
    let addr = lis.local_addr()?;
    let mut clt = TcpStream::connect(addr).await?;
    let (stream, ..) = lis.accept().await?;
    let (rx, tx) = stream.into_split();
    let mut lis =
        Listener::with_decoders(tx, rx, HeaderDecoder::new(2, 3), FrameDecoder::default());
    let mut buf = BytesMut::default();
    HeaderEncoder.encode(
        HeaderRef {
            instance: "foo",
            name: "bar",
        },
        &mut buf,
    )?;
    clt.write_all(&buf).await?;
    let res = lis.accept().await;
    let err = res.err().context("too long instance name accepted")?;
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    let res = lis.accept().await;
    assert!(res.is_err(), "failed connection reused");
    Ok(())
}
