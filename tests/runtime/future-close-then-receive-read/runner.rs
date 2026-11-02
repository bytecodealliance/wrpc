//@ wasmtime-flags = '-Wcomponent-model-async'

use tokio::join;
use tokio::sync::oneshot;

use crate::client::a::b::the_test::{get, set};

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = oneshot::channel::<()>();
    let (res, ()) = join!(
        set(
            wrpc,
            (),
            Box::pin(async move {
                let _ = rx.await;
            }),
        ),
        async { drop(tx) },
    );
    res?;
    let (rx, io) = get(wrpc, ()).await?;
    drop(rx);
    drop(io);
    Ok(())
}
