//@ wasmtime-flags = '-Wcomponent-model-async'

use tokio::join;
use tokio::sync::oneshot;

use crate::client::my::test::i::*;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = oneshot::channel();
    let (res, ()) = join!(
        cancel_before_read(wrpc, (), Box::pin(async move { rx.await.unwrap_or(0) })),
        async { drop(tx) },
    );
    res?;

    let (tx, rx) = oneshot::channel();
    let (res, ()) = join!(
        cancel_after_read(wrpc, (), Box::pin(async move { rx.await.unwrap_or(0) })),
        async { drop(tx) },
    );
    res?;

    let (data_tx, data_rx) = oneshot::channel();
    let (signal_tx, signal_rx) = oneshot::channel();
    let (res, ()) = join!(
        start_read_then_cancel(
            wrpc,
            (),
            Box::pin(async move { data_rx.await.unwrap() }),
            Box::pin(async move { signal_rx.await.unwrap() }),
        ),
        async {
            signal_tx.send(()).unwrap();
            data_tx.send(4).unwrap();
        },
    );
    res?;
    Ok(())
}
