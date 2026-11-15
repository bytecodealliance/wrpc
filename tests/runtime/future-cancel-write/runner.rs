//@ wasmtime-flags = '-Wcomponent-model-async'

use tokio::join;
use tokio::sync::oneshot;

use crate::client::my::test::i::{read_and_drop, take_then_drop};

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    // cancel from the other end
    let (tx, rx) = oneshot::channel();
    let f1 = async { tx.send(String::from("hello")) };
    let (wrpc_tx, wrpc_rx) = wrpc.connect().await?;
    let f2 = take_then_drop(wrpc_tx, wrpc_rx, Box::pin(async move { rx.await.unwrap() }));
    let (result, res) = join!(f1, f2);
    res?;
    result.unwrap();

    // Start a write, then go complete the write in some async work and
    // witness that it was written, not cancelled.
    let (tx, rx) = oneshot::channel();
    tx.send(String::from("hello3")).unwrap();
    let (wrpc_tx, wrpc_rx) = wrpc.connect().await?;
    read_and_drop(wrpc_tx, wrpc_rx, Box::pin(async move { rx.await.unwrap() })).await?;
    Ok(())
}
