//@ wasmtime-flags = '-Wcomponent-model-async'

use tokio::join;
use tokio::sync::oneshot;

use crate::client::my::test::i::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let mut wrpc_clt = wrpc.connect().await?;
    let (tx, rx) = oneshot::channel();
    let (res, ()) = join!(
        read_future(&mut wrpc_clt, Box::pin(async move { rx.await.unwrap() })),
        async { tx.send(()).unwrap() },
    );
    res?;

    let (tx, rx) = oneshot::channel();
    let (res, ()) = join!(
        drop_future(&mut wrpc_clt, Box::pin(async move { rx.await.unwrap() })),
        async { tx.send(()).unwrap() },
    );
    res?;
    Ok(())
}
