use core::pin::{Pin, pin};

use anyhow::Context as _;
use bytes::{Bytes, BytesMut};
use futures::{Stream, StreamExt as _, stream};
use tokio::join;
use tokio::task::yield_now;
use tokio_util::codec::Encoder as _;
use wrpc_transport::{Connect as _, HeaderEncoder, HeaderRef, accept, invoke_values_blocking};

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn loopback() -> anyhow::Result<()> {
    wrpc_test::serve_webtransport(async |clt, srv| {
        wrpc_test::assert_single_invocation((), &clt, srv.incoming()).await?;
        wrpc_test::assert_invalid_data_resets((), &clt, srv.incoming()).await
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn reader_dropped() -> anyhow::Result<()> {
    wrpc_test::with_webtransport(|cc, sc| async move {
        let (mut tx, _rx) = cc.open_bi().await.context("failed to open stream")?;
        let mut buf = BytesMut::default();
        HeaderEncoder
            .encode(
                HeaderRef {
                    instance: "foo",
                    name: "bar",
                },
                &mut buf,
            )
            .context("failed to encode invocation")?;
        tx.write_all(&buf)
            .await
            .context("failed to write invocation")?;
        let srv = wrpc_webtransport::Session::from(sc);
        let mut srv = pin!(srv.incoming());
        let (.., outgoing, incoming) =
            wrpc_test::accept_framed(&mut srv, "foo", "bar", [[None; 0]; 0]).await?;
        drop(incoming);
        drop(outgoing);
        let stopped = tx.stopped().await.context("failed to await stream stop")?;
        assert_eq!(stopped, Some(1));
        Ok(())
    })
    .await
}

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn stopped_mid_params() -> anyhow::Result<()> {
    wrpc_test::with_webtransport(|cc, sc| async move {
        let clt = wrpc_webtransport::Session::from(cc);
        let param: Pin<Box<dyn Stream<Item = Bytes> + Send>> = Box::pin(
            stream::repeat(Bytes::from_static(b"x")).then(|chunk| async {
                yield_now().await;
                chunk
            }),
        );
        let (res, srv) = join!(
            async {
                let (tx, rx) = clt.connect(()).await.expect("failed to connect");
                invoke_values_blocking::<_, (u32,)>(tx, rx, "foo", "bar", (param,)).await
            },
            async {
                let (mut stx, mut srx) = sc.accept_bi().await.context("failed to accept stream")?;
                srx.read_exact(&mut [0; 1])
                    .await
                    .context("failed to read invocation")?;
                srx.stop(0).context("failed to stop stream")?;
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
    wrpc_test::with_webtransport(|cc, sc| async move {
        let (mut tx, _rx) = cc.open_bi().await.context("failed to open stream")?;
        let mut buf = BytesMut::default();
        HeaderEncoder
            .encode(
                HeaderRef {
                    instance: "foo",
                    name: "bar",
                },
                &mut buf,
            )
            .context("failed to encode invocation")?;
        tx.write_all(&buf)
            .await
            .context("failed to write invocation")?;
        let srv = wrpc_webtransport::Session::from(sc);
        let mut srv = pin!(srv.incoming());
        let res = srv.next().await.context("invocation stream ended")?;
        let ((), stx, mut srx) = res.context("failed to accept stream")?;
        accept(&mut srx, &mut BytesMut::default())
            .await
            .context("failed to read invocation header")?;
        drop(stx);
        drop(srx);
        let stopped = tx.stopped().await.context("failed to await stream stop")?;
        assert_eq!(stopped, Some(1));
        Ok(())
    })
    .await
}
