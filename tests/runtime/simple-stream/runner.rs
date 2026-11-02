//@ wasmtime-flags = '-Wcomponent-model-async'

use futures::SinkExt as _;
use futures::channel::mpsc;
use tokio::join;

use crate::client::my::test::i::*;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (mut tx, rx) = mpsc::channel(0);
    let (res, ()) = join!(read_stream(wrpc, (), Box::pin(rx)), async {
        // write one item
        tx.send(vec![()]).await.unwrap();
        // write two items
        tx.send(vec![(), ()]).await.unwrap();
        drop(tx);
    });
    res?;
    Ok(())
}
