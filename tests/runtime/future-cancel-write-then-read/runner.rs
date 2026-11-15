//@ wasmtime-flags = '-Wcomponent-model-async'

use tokio::sync::oneshot;

use crate::client::a::b::the_test::f;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = oneshot::channel();

    drop(async move { tx.send(4).unwrap() });

    let (wrpc_tx, wrpc_rx) = wrpc.connect().await?;
    f(
        wrpc_tx,
        wrpc_rx,
        Box::pin(async move {
            let res = rx.await;
            res.unwrap_or(0)
        }),
    )
    .await
}
