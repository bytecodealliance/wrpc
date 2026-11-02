use core::time::Duration;

use anyhow::Context as _;
use bytes::{Bytes, BytesMut};
use futures::{SinkExt as _, StreamExt as _};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::join;
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::Encoder as _;
use wrpc_transport::frame::{Incoming, Server};
use wrpc_transport::{FrameEncoder, FrameRef};
use wrpc_websockets::tokio_websockets::{
    ClientBuilder, CloseCode, MaybeTlsStream, Message, ServerBuilder, WebSocketStream,
};
use wrpc_websockets::{Client, split};

async fn connect(
    clt: &ClientBuilder<'_>,
    srv: &ServerBuilder,
    lis: &TcpListener,
) -> anyhow::Result<(
    WebSocketStream<TcpStream>,
    WebSocketStream<MaybeTlsStream<TcpStream>>,
)> {
    let (accepted, connected) = join!(
        async {
            let (stream, ..) = lis
                .accept()
                .await
                .context("failed to accept TCP connection")?;
            srv.accept(stream)
                .await
                .context("failed to perform WebSocket handshake")
        },
        clt.connect(),
    );
    let (_req, ws) = accepted?;
    let (conn, _res) = connected.context("failed to connect")?;
    Ok((ws, conn))
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn loopback() -> anyhow::Result<()> {
    let srv = Server::default();
    wrpc_test::with_websockets(|clt, server, lis| async move {
        wrpc_test::assert_single_invocation((), &Client::from(clt), &srv, async {
            let (stream, addr) = lis
                .accept()
                .await
                .context("failed to accept TCP connection")?;
            assert!(addr.ip().is_loopback());
            let (_req, ws) = server
                .accept(stream)
                .await
                .context("failed to perform WebSocket handshake")?;
            let (tx, rx) = split(ws);
            srv.accept((), tx, rx)
                .await
                .context("failed to accept invocation")
        })
        .await
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn writer_dropped() -> anyhow::Result<()> {
    wrpc_test::with_websockets(|clt, server, lis| async move {
        for shutdown in [true, false] {
            let (ws, conn) = connect(&clt, &server, &lis).await?;
            let (mut tx, mut crx) = split(conn);
            let mut frame = BytesMut::default();
            FrameEncoder.encode(
                FrameRef {
                    path: &[],
                    data: b"\x2a",
                },
                &mut frame,
            )?;
            tx.write_all(&frame).await.context("failed to write")?;
            tx.flush().await.context("failed to flush")?;
            if shutdown {
                tx.shutdown().await.context("failed to shut down")?;
            }
            drop(tx);
            if !shutdown {
                let err = crx.read_u8().await.unwrap_err();
                assert_eq!(err.kind(), std::io::ErrorKind::ConnectionAborted);
            }
            let (_tx, rx) = split(ws);
            let mut rx = Incoming::new(rx, [[Some(0)]]);
            let finished = rx.finished();
            let mut buf = Vec::default();
            let res = tokio::time::timeout(Duration::from_secs(5), rx.read_to_end(&mut buf))
                .await
                .context("read timed out")?;
            assert_eq!(buf, b"\x2a");
            drop(rx);
            drop(crx);
            let finished = tokio::time::timeout(Duration::from_secs(5), finished)
                .await
                .context("ingress timed out")?;
            if shutdown {
                res.context("failed to read")?;
                finished.context("ingress failed")?;
            } else {
                let err = res.unwrap_err();
                assert_eq!(err.kind(), std::io::ErrorKind::ConnectionAborted);
                let err = finished.unwrap_err();
                assert_eq!(err.kind(), std::io::ErrorKind::ConnectionAborted);
            }
        }
        Ok(())
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn reader_dropped() -> anyhow::Result<()> {
    async fn assert_closed(
        conn: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
        code: CloseCode,
    ) -> anyhow::Result<()> {
        let msg = conn.next().await.context("EOF sentinel missing")?;
        let msg = msg?;
        assert!(msg.is_text() && msg.as_payload().is_empty());
        let msg = tokio::time::timeout(Duration::from_secs(5), conn.next())
            .await
            .context("connection was not closed")?;
        let msg = msg.context("close missing")?;
        let msg = msg?;
        let close = msg.as_close().context("expected close")?;
        assert_eq!(close.0, code);
        Ok(())
    }

    wrpc_test::with_websockets(|clt, server, lis| async move {
        let (ws, conn) = connect(&clt, &server, &lis).await?;
        let (mut ctx, mut crx) = split(conn);
        ctx.shutdown().await.context("failed to shut down")?;
        let (mut stx, srx) = split(ws);
        drop(srx);
        let data = b"bar".repeat(1 << 20);
        let mut buf = Vec::default();
        let (written, read) = join!(
            async {
                stx.write_all(&data).await?;
                stx.shutdown().await?;
                drop(stx);
                std::io::Result::Ok(())
            },
            crx.read_to_end(&mut buf),
        );
        written.context("failed to write")?;
        read.context("failed to read")?;
        assert_eq!(buf, data);

        for (msg, code) in [
            (Message::binary(&b"foo"[..]), CloseCode::PROTOCOL_ERROR),
            (Message::text(Bytes::new()), CloseCode::NORMAL_CLOSURE),
        ] {
            let (ws, mut conn) = connect(&clt, &server, &lis).await?;
            conn.send(msg).await.context("failed to send message")?;
            ws.get_ref()
                .readable()
                .await
                .context("failed to wait for message")?;
            let (mut stx, srx) = split(ws);
            drop(srx);
            stx.shutdown().await.context("failed to shut down")?;
            drop(stx);
            assert_closed(&mut conn, code).await?;
        }

        let (ws, mut conn) = connect(&clt, &server, &lis).await?;
        conn.send(Message::binary(&b"foo"[..]))
            .await
            .context("failed to send message")?;
        let (mut stx, mut srx) = split(ws);
        let mut buf = [0; 1];
        srx.read_exact(&mut buf).await.context("failed to read")?;
        assert_eq!(&buf, b"f");
        drop(srx);
        stx.shutdown().await.context("failed to shut down")?;
        drop(stx);
        assert_closed(&mut conn, CloseCode::PROTOCOL_ERROR).await
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn normal_close() -> anyhow::Result<()> {
    async fn assert_next(
        conn: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
        f: impl FnOnce(&Message) -> bool,
    ) -> anyhow::Result<()> {
        let msg = tokio::time::timeout(Duration::from_secs(5), conn.next())
            .await
            .context("receive timed out")?;
        let msg = msg.context("connection closed")?;
        let msg = msg?;
        assert!(f(&msg), "{msg:?}");
        Ok(())
    }

    fn is_normal_close(msg: &Message) -> bool {
        msg.as_close()
            .is_some_and(|(code, ..)| code == CloseCode::NORMAL_CLOSURE)
    }

    wrpc_test::with_websockets(|clt, server, lis| async move {
        let (ws, mut conn) = connect(&clt, &server, &lis).await?;
        let (mut tx, mut rx) = split(ws);
        conn.send(Message::binary(&b"foo"[..])).await?;
        conn.send(Message::text(Bytes::new())).await?;
        let mut buf = Vec::default();
        rx.read_to_end(&mut buf).await.context("failed to read")?;
        assert_eq!(buf, b"foo");
        tx.write_all(b"bar").await.context("failed to write")?;
        tx.shutdown().await.context("failed to shut down")?;
        assert_next(&mut conn, |msg| msg.as_payload()[..] == b"bar"[..]).await?;
        assert_next(&mut conn, is_normal_close).await?;

        for eof in [
            Message::text(Bytes::new()),
            Message::close(Some(CloseCode::NORMAL_CLOSURE), ""),
        ] {
            let (ws, mut conn) = connect(&clt, &server, &lis).await?;
            let (mut tx, mut rx) = split(ws);
            tx.shutdown().await.context("failed to shut down")?;
            assert_next(&mut conn, |msg| {
                msg.is_text() && msg.as_payload().is_empty()
            })
            .await?;
            conn.send(Message::binary(&b"foo"[..])).await?;
            conn.send(eof).await?;
            let mut buf = Vec::default();
            rx.read_to_end(&mut buf).await.context("failed to read")?;
            assert_eq!(buf, b"foo");
            assert_next(&mut conn, is_normal_close).await?;
        }
        Ok(())
    })
    .await
}
