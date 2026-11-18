use core::pin::Pin;

use anyhow::Context as _;
use bytes::Bytes;
use futures::{Stream, StreamExt as _, stream};
use quinn::VarInt;
use tokio::join;
use tokio::task::yield_now;
use wrpc_transport::{Decode, Encode, invoke_blocking};

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn loopback() -> anyhow::Result<()> {
    wrpc_test::with_quic(async |clt, srv| {
        let accept = async || srv.accept_bi().await;
        let (tx, rx) = clt.open_bi().await?;
        wrpc_test::assert_single_invocation(tx, rx, accept()).await?;
        wrpc_test::assert_invalid_data_resets(|| clt.open_bi(), accept).await
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn stopped_mid_params() -> anyhow::Result<()> {
    wrpc_test::with_quic(|cc, sc| async move {
        let param: Pin<Box<dyn Stream<Item = Bytes> + Send>> = Box::pin(
            stream::repeat(Bytes::from_static(b"x")).then(|chunk| async {
                yield_now().await;
                chunk
            }),
        );
        let (res, srv) = join!(
            async {
                let (tx, rx) = cc.open_bi().await.expect("failed to open stream");
                invoke_blocking(
                    tx,
                    rx,
                    "foo",
                    "bar",
                    <(Pin<Box<dyn Stream<Item = Bytes> + Send>>,) as Encode>::Encoder::default(),
                    (param,),
                    <(u32,) as Decode>::Decoder::default(),
                )
                .await
            },
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
