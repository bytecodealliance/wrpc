use core::pin::Pin;

use std::sync::Arc;

use anyhow::Context as _;
use bytes::{Bytes, BytesMut};
use futures::{Stream, StreamExt as _, stream};
use quinn::VarInt;
use tokio::join;
use tokio::task::yield_now;
use wrpc_transport::frame::encode_invocation;
use wrpc_transport::{InvokeExt as _, Serve as _};

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn loopback() -> anyhow::Result<()> {
    wrpc_test::serve_quic(async |clt, srv| {
        wrpc_test::assert_single_invocation((), &clt, srv).await?;
        wrpc_test::assert_invalid_data_resets((), &clt, srv).await
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn reader_dropped() -> anyhow::Result<()> {
    let srv = wrpc_quic::Server::default();
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
        wrpc_quic::accept(&srv, stx, srx)
            .await
            .context("failed to accept invocation")?;
        let ((), outgoing, incoming) = invocations
            .next()
            .await
            .context("invocation stream unexpectedly finished")?
            .context("failed to accept invocation")?;
        drop(incoming);
        drop(outgoing);
        let stopped = tx.stopped().await.context("failed to await stream stop")?;
        assert_eq!(stopped, Some(VarInt::from_u32(1)));
        Ok(())
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn stopped_mid_params() -> anyhow::Result<()> {
    wrpc_test::with_quic(|cc, sc| async move {
        let clt = wrpc_quic::Client::from(cc);
        let param: Pin<Box<dyn Stream<Item = Bytes> + Send>> = Box::pin(
            stream::repeat(Bytes::from_static(b"x")).then(|chunk| async {
                yield_now().await;
                chunk
            }),
        );
        let (res, srv) = join!(
            clt.invoke_values_blocking::<_, _, (u32,)>((), "foo", "bar", (param,), [[None; 0]; 0]),
            async {
                let (mut stx, mut srx) = sc.accept_bi().await.context("failed to accept stream")?;
                srx.read_exact(&mut [0; 1])
                    .await
                    .context("failed to read invocation")?;
                srx.stop(VarInt::from_u32(0))
                    .context("failed to stop stream")?;
                stx.write_all(b"\0\x01\x2a\0\0")
                    .await
                    .context("failed to write results")?;
                stx.finish().context("failed to finish stream")?;
                anyhow::Ok(())
            }
        );
        srv?;
        res.expect_err("stopping the parameters must fail the invocation");
        Ok(())
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn unhandled_function() -> anyhow::Result<()> {
    let srv = wrpc_quic::Server::default();
    wrpc_test::with_quic(|cc, sc| async move {
        let (mut tx, _rx) = cc.open_bi().await.context("failed to open stream")?;
        let mut buf = BytesMut::default();
        encode_invocation(&mut buf, "foo", "bar", 0).context("failed to encode invocation")?;
        tx.write_all(&buf)
            .await
            .context("failed to write invocation")?;
        let (stx, srx) = sc.accept_bi().await.context("failed to accept stream")?;
        wrpc_quic::accept(&srv, stx, srx)
            .await
            .expect_err("function must not be handled");
        let stopped = tx.stopped().await.context("failed to await stream stop")?;
        assert_eq!(stopped, Some(VarInt::from_u32(1)));
        Ok(())
    })
    .await
}
