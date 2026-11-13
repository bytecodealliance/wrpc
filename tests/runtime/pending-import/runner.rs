//@ wasmtime-flags = '-Wcomponent-model-async'

use core::future::Future as _;
use core::pin::pin;
use core::task::Context;

use futures::task::noop_waker_ref;
use tokio::sync::oneshot;
use tokio::task::yield_now;

use crate::client::my::test::i::*;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Connect<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    // Test that Rust-level polling twice works.
    {
        let (tx, rx) = oneshot::channel();
        let mut import = pin!(pending_import(
            wrpc,
            (),
            Box::pin(async move { rx.await.unwrap() })
        ));
        assert!(
            import
                .as_mut()
                .poll(&mut Context::from_waker(noop_waker_ref()))
                .is_pending()
        );
        assert!(
            import
                .as_mut()
                .poll(&mut Context::from_waker(noop_waker_ref()))
                .is_pending()
        );
        tx.send(()).unwrap();
        import.await?;
    }

    // Start the imported function call, get it pending, then let it complete by
    // finishing `tx`, then yield a few times to ensure that the runtime gets
    // the completion of the task-at-hand, and then drop it without completing
    // it.
    {
        let (tx, rx) = oneshot::channel();
        let mut import = Box::pin(pending_import(
            wrpc,
            (),
            Box::pin(async move { rx.await.unwrap() }),
        ));
        assert!(
            import
                .as_mut()
                .poll(&mut Context::from_waker(noop_waker_ref()))
                .is_pending()
        );
        tx.send(()).unwrap();
        for _ in 0..5 {
            yield_now().await;
        }
        drop(import);
    }
    Ok(())
}
