use core::future::Future;

use anyhow::Context as _;
use bytes::Bytes;
use futures::{try_join, Stream, StreamExt as _};
use tracing::instrument;
use wrpc_transport::{Acceptor, IncomingInputStream, Value};

pub trait Eventual: wrpc_transport::Client {
    #[instrument(level = "trace", skip_all)]
    fn invoke_delete(
        &self,
        bucket: &str,
        key: &str,
    ) -> impl Future<Output = anyhow::Result<(Result<(), String>, Self::Transmission)>> + Send {
        self.invoke_static("wrpc:keyvalue/eventual@0.1.0", "delete", (bucket, key))
    }

    #[instrument(level = "trace", skip_all)]
    fn serve_delete(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            Self::InvocationStream<
                Self::Context,
                (String, String),
                <Self::Acceptor as Acceptor>::Transmitter,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:keyvalue/eventual@0.1.0", "delete")
    }

    #[instrument(level = "trace", skip_all)]
    fn invoke_exists(
        &self,
        bucket: &str,
        key: &str,
    ) -> impl Future<Output = anyhow::Result<(Result<bool, String>, Self::Transmission)>> + Send
    {
        self.invoke_static("wrpc:keyvalue/eventual@0.1.0", "exists", (bucket, key))
    }

    #[instrument(level = "trace", skip_all)]
    fn serve_exists(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            Self::InvocationStream<
                Self::Context,
                (String, String),
                <Self::Acceptor as Acceptor>::Transmitter,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:keyvalue/eventual@0.1.0", "exists")
    }

    #[instrument(level = "trace", skip_all)]
    fn invoke_get(
        &self,
        bucket: &str,
        key: &str,
    ) -> impl Future<
        Output = anyhow::Result<(
            Result<Option<IncomingInputStream>, String>,
            Self::Transmission,
        )>,
    > + Send {
        self.invoke_static("wrpc:keyvalue/eventual@0.1.0", "get", (bucket, key))
    }

    #[instrument(level = "trace", skip_all)]
    fn serve_get(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            Self::InvocationStream<
                Self::Context,
                (String, String),
                <Self::Acceptor as Acceptor>::Transmitter,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:keyvalue/eventual@0.1.0", "get")
    }

    #[instrument(level = "trace", skip_all)]
    fn invoke_set(
        &self,
        bucket: &str,
        key: &str,
        value: impl Stream<Item = Bytes> + Send + 'static,
    ) -> impl Future<Output = anyhow::Result<(Result<(), String>, Self::Transmission)>> + Send {
        self.invoke_static(
            "wrpc:keyvalue/eventual@0.1.0",
            "set",
            (
                bucket,
                key,
                Value::Stream(Box::pin(
                    value.map(|buf| Ok(buf.into_iter().map(Value::U8).map(Some).collect())),
                )),
            ),
        )
    }

    #[instrument(level = "trace", skip_all)]
    fn serve_set(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            Self::InvocationStream<
                Self::Context,
                (String, String, IncomingInputStream),
                <Self::Acceptor as Acceptor>::Transmitter,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:keyvalue/eventual@0.1.0", "set")
    }
}

impl<T: wrpc_transport::Client> Eventual for T {}

pub trait Atomic: wrpc_transport::Client {
    #[instrument(level = "trace", skip_all)]
    fn invoke_compare_and_swap(
        &self,
        bucket: &str,
        key: &str,
        old: u64,
        new: u64,
    ) -> impl Future<Output = anyhow::Result<(Result<bool, String>, Self::Transmission)>> + Send
    {
        self.invoke_static(
            "wrpc:keyvalue/atomic@0.1.0",
            "compare-and-swap",
            (bucket, key, old, new),
        )
    }

    #[instrument(level = "trace", skip_all)]
    fn serve_compare_and_swap(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            Self::InvocationStream<
                Self::Context,
                (String, String, u64, u64),
                <Self::Acceptor as Acceptor>::Transmitter,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:keyvalue/atomic@0.1.0", "compare-and-swap")
    }

    #[instrument(level = "trace", skip_all)]
    fn invoke_increment(
        &self,
        bucket: &str,
        key: &str,
        delta: u64,
    ) -> impl Future<Output = anyhow::Result<(Result<u64, String>, Self::Transmission)>> + Send
    {
        self.invoke_static(
            "wrpc:keyvalue/atomic@0.1.0",
            "increment",
            (bucket, key, delta),
        )
    }

    #[instrument(level = "trace", skip_all)]
    fn serve_increment(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            Self::InvocationStream<
                Self::Context,
                (String, String, u64),
                <Self::Acceptor as Acceptor>::Transmitter,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:keyvalue/atomic@0.1.0", "increment")
    }
}

impl<T: wrpc_transport::Client> Atomic for T {}

/// `wrpc:keyvalue/atomic` invocation streams
pub struct AtomicInvocations<T>
where
    T: wrpc_transport::Client,
{
    pub compare_and_swap: T::InvocationStream<
        T::Context,
        (String, String, u64, u64),
        <T::Acceptor as Acceptor>::Transmitter,
    >,
    pub increment: T::InvocationStream<
        T::Context,
        (String, String, u64),
        <T::Acceptor as Acceptor>::Transmitter,
    >,
}

/// Serve `wrpc:keyvalue/atomic` invocations
#[instrument(level = "trace", skip_all)]
pub async fn serve_atomic<T>(client: &T) -> anyhow::Result<AtomicInvocations<T>>
where
    T: Atomic,
{
    let (compare_and_swap, increment) = try_join!(
        async {
            client
                .serve_compare_and_swap()
                .await
                .context("failed to serve `wrpc:keyvalue/atomic.compare-and-swap`")
        },
        async {
            client
                .serve_increment()
                .await
                .context("failed to serve `wrpc:keyvalue/atomic.increment`")
        },
    )?;
    Ok(AtomicInvocations {
        compare_and_swap,
        increment,
    })
}

/// `wrpc:keyvalue/eventual` invocation streams
pub struct EventualInvocations<T>
where
    T: wrpc_transport::Client,
{
    pub delete:
        T::InvocationStream<T::Context, (String, String), <T::Acceptor as Acceptor>::Transmitter>,
    pub exists:
        T::InvocationStream<T::Context, (String, String), <T::Acceptor as Acceptor>::Transmitter>,
    pub get:
        T::InvocationStream<T::Context, (String, String), <T::Acceptor as Acceptor>::Transmitter>,
    pub set: T::InvocationStream<
        T::Context,
        (String, String, IncomingInputStream),
        <T::Acceptor as Acceptor>::Transmitter,
    >,
}

/// Serve `wrpc:keyvalue/eventual` invocations
#[instrument(level = "trace", skip_all)]
pub async fn serve_eventual<T>(client: &T) -> anyhow::Result<EventualInvocations<T>>
where
    T: Eventual,
{
    let (delete, exists, get, set) = try_join!(
        async {
            client
                .serve_delete()
                .await
                .context("failed to serve `wrpc:keyvalue/eventual.delete`")
        },
        async {
            client
                .serve_exists()
                .await
                .context("failed to serve `wrpc:keyvalue/eventual.exists`")
        },
        async {
            client
                .serve_get()
                .await
                .context("failed to serve `wrpc:keyvalue/eventual.get`")
        },
        async {
            client
                .serve_set()
                .await
                .context("failed to serve `wrpc:keyvalue/eventual.set`")
        },
    )?;
    Ok(EventualInvocations {
        delete,
        exists,
        get,
        set,
    })
}
