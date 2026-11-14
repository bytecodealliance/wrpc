//@ wasmtime-flags = '-Wcomponent-model-async'

use tokio::sync::oneshot;
use tokio::{join, try_join};

use crate::client::a::b::the_test::f;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Connect<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = oneshot::channel();
    let a = async { tx.send(()).unwrap() };
    let b = async {
        let (rx, io) = f(wrpc, (), Box::pin(async move { rx.await.unwrap() })).await?;
        try_join!(async { Ok(rx.await) }, io)?;
        ::wit_bindgen_wrpc::anyhow::Ok(())
    };
    let ((), b_result) = join!(a, b);
    b_result
}
