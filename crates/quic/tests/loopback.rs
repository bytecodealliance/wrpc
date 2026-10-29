use std::sync::Arc;

use anyhow::Context as _;
use bytes::BytesMut;
use futures::StreamExt as _;
use quinn::VarInt;
use tokio::io::AsyncWriteExt as _;
use wrpc_quic::Client;
use wrpc_transport::Serve as _;
use wrpc_transport::frame::encode_invocation;

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn loopback() -> anyhow::Result<()> {
    let srv = wrpc_quic::Server::new();
    wrpc_test::with_quic(|cc, sc| async move {
        wrpc_test::assert_single_invocation((), &Client::from(cc), &srv, async {
            let (tx, rx) = sc.accept_bi().await.context("failed to accept stream")?;
            srv.accept((), tx.into(), rx.into())
                .await
                .context("failed to accept invocation")
        })
        .await
    })
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

        let (stx, srx) = sc.accept_bi().await.context("failed to accept stream")?;
        srv.accept((), stx.into(), srx.into())
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
        for shutdown in [true, false] {
            let (tx, _rx) = cc.open_bi().await.context("failed to open stream")?;
            let mut tx = wrpc_quic::SendStream::from(tx);
            tx.write_all(b"foo")
                .await
                .context("failed to write to stream")?;
            if shutdown {
                tx.shutdown().await.context("failed to shut down stream")?;
            }
            drop(tx);
            let (_stx, mut srx) = sc.accept_bi().await.context("failed to accept stream")?;
            let res = srx.read_to_end(usize::MAX).await;
            if shutdown {
                assert_eq!(res.context("failed to read stream")?, b"foo");
            } else {
                assert!(
                    matches!(
                        res,
                        Err(quinn::ReadToEndError::Read(quinn::ReadError::Reset(code)))
                            if code == VarInt::from_u32(0)
                    ),
                    "{res:?}"
                );
            }
        }
        Ok(())
    })
    .await
}
