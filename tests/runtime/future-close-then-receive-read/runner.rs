//@ wasmtime-flags = '-Wcomponent-model-async'

use tokio::join;
use tokio::sync::oneshot;

use crate::client::a::b::the_test::{get, set};

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = oneshot::channel::<()>();
    let (wrpc_tx, wrpc_rx) = wrpc.connect().await?;
    let (res, ()) = join!(
        set(
            wrpc_tx,
            wrpc_rx,
            Box::pin(async move {
                let _ = rx.await;
            }),
        ),
        async { drop(tx) },
    );
    res?;
    let (wrpc_tx, wrpc_rx) = wrpc.connect().await?;
    let (rx, io) = get(wrpc_tx, wrpc_rx).await?;
    drop(rx);
    drop(io);
    Ok(())
}
