use anyhow::Context as _;
use wrpc_transport::frame::Server;
use wrpc_websockets::{Client, split};

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn loopback() -> anyhow::Result<()> {
    let srv = Server::default();
    wrpc_test::with_websockets(|clt, server, lis| async move {
        wrpc_test::assert_single_invocation(&Client::from(clt), &srv, async {
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
