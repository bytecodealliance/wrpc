//@ wasmtime-flags = '-Wcomponent-model-async'

use tokio::join;
use tokio::sync::oneshot;

use crate::client::my::test::i::*;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Connect<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = oneshot::channel();
    let (res, ()) = join!(
        read_future(wrpc, (), Box::pin(async move { rx.await.unwrap() })),
        async { tx.send(()).unwrap() },
    );
    res?;

    let (tx, rx) = oneshot::channel();
    let (res, ()) = join!(
        drop_future(wrpc, (), Box::pin(async move { rx.await.unwrap() })),
        async { tx.send(()).unwrap() },
    );
    res?;
    Ok(())
}
