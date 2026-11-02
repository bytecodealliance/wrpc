//@ wasmtime-flags = '-Wcomponent-model-async'

use tokio::join;
use tokio::sync::oneshot;

use crate::client::a::b::the_test::f;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = oneshot::channel();
    let a = async { tx.send(()).unwrap() };
    let b = f(wrpc, (), Box::pin(async move { rx.await.unwrap() }));
    let ((), b_result) = join!(a, b);
    b_result
}
