//@ wasmtime-flags = '-Wcomponent-model-async'

use tokio::task::yield_now;
use tokio::try_join;

use crate::client::my::test::i::*;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    try_join!(pending_import(wrpc, ()), async {
        // Ensure that the above future has hit its pending state by
        // spinning a few times here to guarantee that we've yielded to
        // it.
        for _ in 0..5 {
            yield_now().await;
        }
        resolve_pending_import(wrpc, ()).await
    })?;
    Ok(())
}
