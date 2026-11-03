//@ wasmtime-flags = '-Wcomponent-model-async'

use core::future;

use tokio::join;
use tokio::sync::oneshot;

use crate::client::a::b::the_test::f;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = oneshot::channel();
    let a = future::ready(tx.send(()));
    let b = f(wrpc, (), Box::pin(async move { rx.await.unwrap() }));
    let (a_result, b_result) = join!(a, b);
    a_result.unwrap();
    b_result
}
