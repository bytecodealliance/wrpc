use anyhow::Context;
use bytes::Bytes;
use futures::{stream, StreamExt as _};
use tokio::join;
use tracing::info;

#[allow(dead_code)]
pub async fn assert_async<C: Default>(
    client: &impl wrpc_transport::Invoke<Context = C>,
) -> anyhow::Result<()> {
    wit_bindgen_wrpc::generate!({
        world: "async-client",
        path: "tests/wit",
    });

    info!("calling `wrpc-test:integration/async.with-streams`");
    let ((a, b), io) = wrpc_test::integration::async_::with_streams(client, C::default())
        .await
        .context("failed to call `wrpc-test:integration/async.with-streams`")?;
    join!(
        async {
            info!("receiving `a`");
            assert_eq!(a.collect::<Vec<Bytes>>().await.concat(), b"test");
        },
        async {
            info!("receiving `b`");
            assert_eq!(
                b.collect::<Vec<_>>().await.concat(),
                [["foo"].as_slice(), ["bar", "baz"].as_slice()]
            );
        },
        async {
            if let Some(io) = io {
                info!("performing I/O");
                io.await.expect("failed to complete async I/O");
            }
        }
    );

    info!("calling `wrpc-test:integration/async.with-future`");
    let (fut, io) = wrpc_test::integration::async_::with_future(
        client,
        C::default(),
        &wrpc_test::integration::async_::Something {
            foo: "bar".to_string(),
        },
        Box::pin(stream::iter(["foo".into(), "bar".into()])),
    )
    .await
    .context("failed to call `wrpc-test:integration/async.with-future`")?;
    join!(
        async {
            info!("receiving results");
            assert_eq!(fut.await.collect::<Vec<Bytes>>().await.concat(), b"foobar");
        },
        async {
            if let Some(io) = io {
                info!("performing I/O");
                io.await.expect("failed to complete async I/O");
            }
        }
    );

    Ok(())
}
