//@ wasmtime-flags = '-Wcomponent-model-async'

use ::wit_bindgen_wrpc::bytes::Bytes;
use futures::SinkExt as _;
use futures::channel::mpsc;
use tokio::join;

use crate::client::my::test::i::*;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Connect<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (mut tx, rx) = mpsc::channel(0);
    let (res, ()) = join!(read_stream(wrpc, (), Box::pin(rx)), async {
        tx.send(Bytes::from_static(&[10, 20, 30])).await.unwrap();
        // Drop the writer so the reader sees the end of the stream.
        drop(tx);
    });
    res?;
    Ok(())
}
