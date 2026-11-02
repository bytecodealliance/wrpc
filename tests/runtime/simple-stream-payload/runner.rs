//@ wasmtime-flags = '-Wcomponent-model-async'

use ::wit_bindgen_wrpc::bytes::Bytes;
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
        tx.send(Bytes::from_static(b"\x00")).await.unwrap();
        // write two items
        tx.send(Bytes::from_static(b"\x01\x02")).await.unwrap();
        // write 1/2 items again
        tx.send(Bytes::from_static(b"\x03\x04")).await.unwrap();
        drop(tx);
    });
    res?;
    Ok(())
}
