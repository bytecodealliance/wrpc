//@ wasmtime-flags = '-Wcomponent-model-async'

use futures::SinkExt as _;
use futures::channel::mpsc;
use tokio::join;

use crate::client::my::test::i::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (mut tx, rx) = mpsc::channel(0);
    let (wrpc_tx, wrpc_rx) = wrpc.connect().await?;
    let (res, ()) = join!(read_stream(wrpc_tx, wrpc_rx, Box::pin(rx)), async {
        // write one item
        tx.send(vec![()]).await.unwrap();
        // write two items
        tx.send(vec![(), ()]).await.unwrap();
        drop(tx);
    });
    res?;
    Ok(())
}
