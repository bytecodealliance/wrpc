//@ wasmtime-flags = '-Wcomponent-model-async'

use tokio::join;
use tokio::sync::oneshot;

use crate::client::my::test::i::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = oneshot::channel();
    let (wrpc_tx, wrpc_rx) = wrpc.connect().await?;
    let (res, ()) = join!(
        read_future(wrpc_tx, wrpc_rx, Box::pin(async move { rx.await.unwrap() })),
        async { tx.send(()).unwrap() },
    );
    res?;

    let (tx, rx) = oneshot::channel();
    let (wrpc_tx, wrpc_rx) = wrpc.connect().await?;
    let (res, ()) = join!(
        drop_future(wrpc_tx, wrpc_rx, Box::pin(async move { rx.await.unwrap() })),
        async { tx.send(()).unwrap() },
    );
    res?;
    Ok(())
}
