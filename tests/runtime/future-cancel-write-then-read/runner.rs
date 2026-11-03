//@ wasmtime-flags = '-Wcomponent-model-async'

use tokio::sync::oneshot;

use crate::client::a::b::the_test::f;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = oneshot::channel();

    drop(async move { tx.send(4).unwrap() });

    f(
        wrpc,
        (),
        Box::pin(async move {
            let res = rx.await;
            res.unwrap_or(0)
        }),
    )
    .await
}
