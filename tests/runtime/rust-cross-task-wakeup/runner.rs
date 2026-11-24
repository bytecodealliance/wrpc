//@ wasmtime-flags = '-Wcomponent-model-async'

use tokio::task::yield_now;
use tokio::try_join;

use crate::client::my::test::i::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::anyhow::Result<()> {
    let clt = wrpc.connect().await?;
    try_join!(
        async {
            pending_import(clt).await?;
            ::anyhow::Ok(())
        },
        async {
            // Ensure that the above future has hit its pending state by
            // spinning a few times here to guarantee that we've yielded to
            // it.
            for _ in 0..5 {
                yield_now().await;
            }
            let clt = wrpc.connect().await?;
            resolve_pending_import(clt).await?;
            Ok(())
        }
    )?;
    Ok(())
}
