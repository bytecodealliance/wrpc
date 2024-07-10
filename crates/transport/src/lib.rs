#![allow(clippy::type_complexity)]

#[cfg(feature = "frame")]
pub mod frame;
pub mod invoke;
pub mod serve;

mod value;

#[cfg(feature = "frame")]
pub use frame::{Decoder as FrameDecoder, Encoder as FrameEncoder, FrameRef};
pub use invoke::{Invoke, InvokeExt};
pub use send_future::SendFuture;
pub use serve::{Serve, ServeExt};
pub use value::*;

#[doc(hidden)]
// This is an internal trait used as a workaround for
// https://github.com/rust-lang/rust/issues/63033
pub trait Captures<'a> {}

impl<'a, T: ?Sized> Captures<'a> for T {}

/// `Index` implementations are capable of multiplexing underlying connections using a particular
/// structural `path`
pub trait Index<T> {
    /// Index the entity using a structural `path`
    fn index(&self, path: &[usize]) -> anyhow::Result<T>;
}

#[allow(unused)]
#[cfg(test)]
mod tests {
    use core::future::Future;
    use core::pin::Pin;

    use std::sync::Arc;

    use bytes::Bytes;
    use futures::{stream, Stream, StreamExt as _, TryStreamExt as _};
    use tokio::join;

    use super::*;

    #[allow(clippy::manual_async_fn)]
    fn invoke_values_send<T>() -> impl Future<
        Output = anyhow::Result<(
            Pin<Box<dyn Stream<Item = Vec<Pin<Box<dyn Future<Output = String> + Send>>>> + Send>>,
        )>,
    > + Send
    where
        T: Invoke<Context = ()> + Default,
    {
        async {
            let wrpc = T::default();
            let ((r0,), _) = wrpc
                .invoke_values(
                    (),
                    "wrpc-test:integration/async",
                    "with-streams",
                    (),
                    [[None].as_slice()],
                )
                .send()
                .await?;
            Ok(r0)
        }
    }

    async fn call_invoke<T: Invoke>(
        i: &T,
        cx: T::Context,
        paths: Arc<[Arc<[Option<usize>]>]>,
    ) -> anyhow::Result<(T::Outgoing, T::Incoming)> {
        i.invoke(cx, "foo", "bar", Bytes::default(), &paths).await
    }

    async fn call_invoke_async<T>() -> anyhow::Result<(Pin<Box<dyn Stream<Item = Bytes> + Send>>,)>
    where
        T: Invoke<Context = ()> + Default,
    {
        let wrpc = T::default();
        let ((r0,), _) = wrpc
            .invoke_values(
                (),
                "wrpc-test:integration/async",
                "with-streams",
                (),
                [
                    [Some(1), Some(2)].as_slice(),
                    [None].as_slice(),
                    [Some(42)].as_slice(),
                ],
            )
            .await?;
        Ok(r0)
    }

    trait Handler {
        fn foo() -> impl Future<Output = anyhow::Result<()>>;
    }

    impl<T> Handler for T
    where
        T: Invoke<Context = ()> + Default,
    {
        async fn foo() -> anyhow::Result<()> {
            call_invoke_async::<Self>().await?;
            Ok(())
        }
    }

    async fn call_serve<T: Serve>(
        s: &T,
    ) -> anyhow::Result<Vec<(T::Context, T::Outgoing, T::Incoming)>> {
        let st = stream::empty()
            .chain({
                s.serve(
                    "foo",
                    "bar",
                    [Box::from([Some(42), None]), Box::from([None])],
                )
                .await
                .unwrap()
            })
            .chain({
                s.serve(
                    "foo",
                    "bar",
                    vec![Box::from([Some(42), None]), Box::from([None])],
                )
                .await
                .unwrap()
            })
            .chain({
                s.serve(
                    "foo",
                    "bar",
                    [Box::from([Some(42), None]), Box::from([None])].as_slice(),
                )
                .await
                .unwrap()
            });
        tokio::spawn(async move { st.try_collect().await })
            .await
            .unwrap()
    }
}
