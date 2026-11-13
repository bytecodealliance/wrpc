//@ wasmtime-flags = '-Wcomponent-model-async'

use tokio::sync::oneshot;
use tokio::{join, try_join};

use crate::client::my::test::i::{ping, pong};

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Connect<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = oneshot::channel();
    let f1 = ping(
        wrpc,
        (),
        Box::pin(async move { rx.await.unwrap() }),
        "world",
    );
    let f2 = async { tx.send("hello".to_string()).unwrap() };
    let (res, ()) = join!(f1, f2);
    let (rx2, io) = res?;
    let (m2, ()) = try_join!(async { Ok(rx2.await) }, io)?;
    assert_eq!(m2, "helloworld");

    let (tx, rx) = oneshot::channel();
    let f1 = async {
        let (m3, io) = pong(wrpc, (), Box::pin(async move { rx.await.unwrap() })).await?;
        assert_eq!(m3, "helloworld");
        io.await?;
        ::wit_bindgen_wrpc::anyhow::Ok(())
    };
    let f2 = async { tx.send(m2).unwrap() };
    let (res, ()) = join!(f1, f2);
    res
}
