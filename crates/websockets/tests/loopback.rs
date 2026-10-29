use anyhow::Context as _;
use futures::SinkExt as _;
use tokio::io::AsyncReadExt as _;
use wrpc_transport::frame::{Incoming, Server};
use wrpc_websockets::tokio_websockets::Message;
use wrpc_websockets::{Client, split};

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
