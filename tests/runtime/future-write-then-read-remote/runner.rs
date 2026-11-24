//@ wasmtime-flags = '-Wcomponent-model-async'

use tokio::join;
use tokio::sync::oneshot;

use crate::client::a::b::the_test::f;

pub async fn run(
    wrpc: &crate::Client,
) -> ::anyhow::Result<()> {
    let (tx, rx) = oneshot::channel();
    let a = async { tx.send(()).unwrap() };
    let wrpc_clt = wrpc.connect().await?;
    let b = f(wrpc_clt, Box::pin(async move { rx.await.unwrap() }));
    let ((), b_result) = join!(a, b);
    b_result?;
    Ok(())
}
