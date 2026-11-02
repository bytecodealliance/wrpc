use std::sync::Arc;

use anyhow::Context as _;
use bytes::BytesMut;
use futures::StreamExt as _;
use quinn::VarInt;
use tokio::io::AsyncWriteExt as _;
use wrpc_transport::Serve as _;
use wrpc_transport::frame::encode_invocation;

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn loopback() -> anyhow::Result<()> {
    wrpc_test::serve_quic(async |clt, srv| wrpc_test::assert_single_invocation((), &clt, srv).await)
        .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn params_dropped() -> anyhow::Result<()> {
    let srv = wrpc_quic::Server::new();
    let invocations = srv
        .serve("foo", "bar", Arc::from([]))
        .await
        .context("failed to serve function")?;
    let mut invocations = Box::pin(invocations);
    wrpc_test::with_quic(|cc, sc| async move {
        let (mut tx, _rx) = cc.open_bi().await.context("failed to open stream")?;
        let mut buf = BytesMut::default();
        encode_invocation(&mut buf, "foo", "bar", 0).context("failed to encode invocation")?;
        tx.write_all(&buf)
            .await
            .context("failed to write invocation")?;

        let stream = sc.accept_bi().await.context("failed to accept stream")?;
        let (srx, stx) = wrpc_quic::split(stream);
        srv.accept((), stx, srx)
            .await
            .context("failed to accept invocation")?;
        let ((), _outgoing, incoming) = invocations
            .next()
            .await
            .context("invocation stream unexpectedly finished")?
            .context("failed to accept invocation")?;
        drop(incoming);

        let code = tx.stopped().await.context("failed to await stream stop")?;
        assert_eq!(code, Some(VarInt::from_u32(1)));
        Ok(())
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn send_stream_dropped() -> anyhow::Result<()> {
    wrpc_test::with_quic(|cc, sc| async move {
        let stream = cc.open_bi().await.context("failed to open stream")?;
        let (_rx, mut tx) = wrpc_quic::split(stream);
        tx.write_all(b"foo")
            .await
            .context("failed to write to stream")?;
        drop(tx);
        let (_stx, mut srx) = sc.accept_bi().await.context("failed to accept stream")?;
        let buf = srx
            .read_to_end(usize::MAX)
            .await
            .context("failed to read stream")?;
        assert_eq!(buf, b"foo");
        Ok(())
    })
    .await
}
