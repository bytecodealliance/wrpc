//@ wasmtime-flags = '-Wcomponent-model-async'

use tokio::sync::oneshot;

use crate::client::a::b::the_test::f;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Connect<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = oneshot::channel();

    drop(tx);

    f(
        wrpc,
        (),
        Box::pin(async move {
            let res = rx.await;
            res.unwrap_or_default()
        }),
    )
    .await
}
