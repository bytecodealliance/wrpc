//@ wasmtime-flags = '-Wcomponent-model-async'

use tokio::join;
use tokio::sync::oneshot;

use crate::client::a::b::the_test::{get, set};

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = oneshot::channel::<()>();
    let (mut wrpc_tx, mut wrpc_rx) = wrpc.connect().await?;
    let (res, ()) = join!(
        set(
            &mut wrpc_tx,
            &mut wrpc_rx,
            Box::pin(async move {
                let _ = rx.await;
            }),
        ),
        async { drop(tx) },
    );
    res?;
    let (rx, io) = get(&mut wrpc_tx, &mut wrpc_rx).await?;
    drop(rx);
    drop(io);
    Ok(())
}
