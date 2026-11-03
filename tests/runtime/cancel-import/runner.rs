//@ wasmtime-flags = '-Wcomponent-model-async'

use core::future::Future as _;
use core::task::Context;

use futures::task::noop_waker_ref;
use tokio::sync::oneshot;
use tokio::task::yield_now;

use crate::client::my::test::i::*;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    println!("test cancelling an import in progress");
    {
        let (mut tx, rx) = oneshot::channel();
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
        drop(import);
        tx.closed().await;
        tx.send(()).unwrap_err();
    }

    println!("test cancelling an import before it starts");
    {
        let (mut tx, rx) = oneshot::channel();
        let import = Box::pin(pending_import(
            wrpc,
            (),
            Box::pin(async move { rx.await.unwrap() }),
        ));
        drop(import);
        tx.closed().await;
        tx.send(()).unwrap_err();
    }

    println!("test cancelling an import in the started state");
    {
        let (mut tx1, rx1) = oneshot::channel();
        let (mut tx2, rx2) = oneshot::channel();

        // create a task in the "started" state, but don't complete it yet
        let mut started_import = Box::pin(pending_import(
            wrpc,
            (),
            Box::pin(async move { rx1.await.unwrap() }),
        ));
        assert!(
            started_import
                .as_mut()
                .poll(&mut Context::from_waker(noop_waker_ref()))
                .is_pending()
        );

        // request the other component sets its backpressure flag meaning we
        // won't be able to create new tasks in the "started" state.
        backpressure_set(wrpc, (), true).await?;
        let mut starting_import = Box::pin(pending_import(
            wrpc,
            (),
            Box::pin(async move { rx2.await.unwrap() }),
        ));
        assert!(
            starting_import
                .as_mut()
                .poll(&mut Context::from_waker(noop_waker_ref()))
                .is_pending()
        );

        // Now cancel the "starting" import. This should notably drop handles in
        // arguments since they get re-acquired during cancellation
        drop(starting_import);

        // cancel our in-progress export
        drop(started_import);

        backpressure_set(wrpc, (), false).await?;

        // both channels should be dropped
        tx1.closed().await;
        tx1.send(()).unwrap_err();
        tx2.closed().await;
        tx2.send(()).unwrap_err();
    }

    // Race an import's cancellation with a status code saying it's done.
    println!("test cancellation with a status code saying it's done");
    {
        // Start a subtask and get it into the "started" state
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

        // Complete the subtask, but don't see the completion in Rust yet.
        tx.send(()).unwrap();

        // Let the subtask's completion notification make its way to our task
        // here.
        for _ in 0..5 {
            yield_now().await;
        }

        // Now cancel the import, despite it actually being done. This should
        // realize that the cancellation is racing completion.
        drop(import);
    }

    // Race an import's cancellation with a pending status code indicating that
    // it's transitioning from started => returned.
    println!("race cancellation with pending status code");
    {
        // Start a subtask and get it into the "started" state
        let (tx1, rx1) = oneshot::channel();
        let mut started_import = Box::pin(pending_import(
            wrpc,
            (),
            Box::pin(async move { rx1.await.unwrap() }),
        ));
        assert!(
            started_import
                .as_mut()
                .poll(&mut Context::from_waker(noop_waker_ref()))
                .is_pending()
        );

        // force the next subtask to start out in the "starting" state, not the
        // "started" state.
        backpressure_set(wrpc, (), true).await?;
        let (mut tx2, rx2) = oneshot::channel();
        let mut starting_import = Box::pin(pending_import(
            wrpc,
            (),
            Box::pin(async move { rx2.await.unwrap() }),
        ));
        assert!(
            starting_import
                .as_mut()
                .poll(&mut Context::from_waker(noop_waker_ref()))
                .is_pending()
        );

        // Disable backpressure in the other component which will let the
        // `starting_import`, previously in the "STARTING" state, get a queued up
        // notification that it's entered the "STARTED" state.
        backpressure_set(wrpc, (), false).await?;
        for _ in 0..5 {
            yield_now().await;
        }

        // Now cancel the `starting_import`. This should correctly pick up the
        // STARTING => STARTED state transition and handle that correctly.
        drop(starting_import);

        // Our future to the import we cancelled shouldn't be able to complete
        // its write.
        tx2.closed().await;
        tx2.send(()).unwrap_err();

        // Complete the other import normally just to assert that it's not
        // cancelled and able to proceed as usual.
        tx1.send(()).unwrap();
        started_import.await?;
    }
    Ok(())
}
