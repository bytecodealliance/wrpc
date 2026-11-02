use core::time::Duration;

use anyhow::Context as _;
use bytes::Bytes;
use futures::{SinkExt as _, StreamExt as _};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::join;
use tokio::net::{TcpListener, TcpStream};
use wrpc_transport::frame::{Incoming, Server};
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
async fn close_before_eof() -> anyhow::Result<()> {
    wrpc_test::with_websockets(|clt, server, lis| async move {
        let (accepted, connected) = tokio::join!(
            async {
                let (stream, ..) = lis
                    .accept()
                    .await
                    .context("failed to accept TCP connection")?;
                server
                    .accept(stream)
                    .await
                    .context("failed to perform WebSocket handshake")
            },
            clt.connect(),
        );
        let (_req, ws) = accepted?;
        let (mut clt, _res) = connected.context("failed to connect")?;
        clt.send(Message::binary(&[0x00, 0x01, 0x2a][..]))
            .await
            .context("failed to send message")?;
        drop(clt);
        let (_tx, rx) = split(ws);
        let mut rx = Incoming::new(rx, [[Some(0)]]);
        let finished = rx.finished();
        let mut buf = Vec::default();
        let err = rx
            .read_to_end(&mut buf)
            .await
            .expect_err("reading should fail");
        assert_eq!(err.kind(), std::io::ErrorKind::ConnectionAborted);
        assert_eq!(buf, [0x2a]);
        drop(rx);
        let err = finished.await.expect_err("ingress should fail");
        assert_eq!(err.kind(), std::io::ErrorKind::ConnectionAborted);
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
