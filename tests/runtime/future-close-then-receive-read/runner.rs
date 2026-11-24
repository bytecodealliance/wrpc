//@ wasmtime-flags = '-Wcomponent-model-async'

use tokio::join;
use tokio::sync::oneshot;

use crate::client::a::b::the_test::{get, set};

pub async fn run(
    wrpc: &crate::Client,
) -> ::anyhow::Result<()> {
    let (tx, rx) = oneshot::channel::<()>();
    let mut wrpc_clt = wrpc.connect().await?;
    let (res, ()) = join!(
        set(
            &mut wrpc_clt,
            Box::pin(async move {
                let _ = rx.await;
            }),
        ),
        async { drop(tx) },
    );
    res?;
    let (rx, io) = get(&mut wrpc_clt).await?;
    drop(rx);
    drop(io);
    Ok(())
}
