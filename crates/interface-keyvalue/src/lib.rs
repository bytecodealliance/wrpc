use core::future::Future;
use core::pin::Pin;

use bytes::Bytes;
use futures::{Stream, StreamExt as _};
use tracing::instrument;
use wrpc_transport::{AcceptedInvocation, Acceptor, IncomingInputStream, Value};

type StringStringInvocationStream<T> = Pin<
    Box<
        dyn Stream<
                Item = anyhow::Result<
                    AcceptedInvocation<
                        <T as wrpc_transport::Client>::Context,
                        (String, String),
                        <T as wrpc_transport::Client>::Subject,
                        <<T as wrpc_transport::Client>::Acceptor as Acceptor>::Transmitter,
                    >,
                >,
            > + Send,
    >,
>;

pub trait Eventual: wrpc_transport::Client {
    type DeleteInvocationStream;
    type ExistsInvocationStream;
    type GetInvocationStream;
    type SetInvocationStream;

    fn invoke_delete(
        &self,
        bucket: String,
        key: String,
    ) -> impl Future<Output = anyhow::Result<(Result<(), String>, Self::Transmission)>> + Send;
    fn serve_delete(
        &self,
    ) -> impl Future<Output = anyhow::Result<Self::DeleteInvocationStream>> + Send;

    fn invoke_exists(
        &self,
        bucket: String,
        key: String,
    ) -> impl Future<Output = anyhow::Result<(Result<bool, String>, Self::Transmission)>> + Send;
    fn serve_exists(
        &self,
    ) -> impl Future<Output = anyhow::Result<Self::ExistsInvocationStream>> + Send;

    fn invoke_get(
        &self,
        bucket: String,
        key: String,
    ) -> impl Future<
        Output = anyhow::Result<(
            Result<Option<IncomingInputStream>, String>,
            Self::Transmission,
        )>,
    > + Send;
    fn serve_get(&self) -> impl Future<Output = anyhow::Result<Self::GetInvocationStream>> + Send;

    fn invoke_set(
        &self,
        bucket: String,
        key: String,
        value: impl Stream<Item = Bytes> + Send + 'static,
    ) -> impl Future<Output = anyhow::Result<(Result<(), String>, Self::Transmission)>> + Send;
    fn serve_set(&self) -> impl Future<Output = anyhow::Result<Self::SetInvocationStream>> + Send;
}

impl<T: wrpc_transport::Client> Eventual for T {
    type DeleteInvocationStream = StringStringInvocationStream<T>;
    type ExistsInvocationStream = StringStringInvocationStream<T>;
    type GetInvocationStream = StringStringInvocationStream<T>;
    type SetInvocationStream = Pin<
        Box<
            dyn Stream<
                    Item = anyhow::Result<
                        AcceptedInvocation<
                            T::Context,
                            (String, String, IncomingInputStream),
                            T::Subject,
                            <T::Acceptor as Acceptor>::Transmitter,
                        >,
                    >,
                > + Send,
        >,
    >;

    #[instrument(level = "trace", skip_all)]
    async fn invoke_delete(
        &self,
        bucket: String,
        key: String,
    ) -> anyhow::Result<(Result<(), String>, T::Transmission)> {
        self.invoke_static("wrpc:keyvalue/eventual@0.1.0", "delete", (bucket, key))
            .await
    }
    #[instrument(level = "trace", skip_all)]
    async fn serve_delete(&self) -> anyhow::Result<Self::DeleteInvocationStream> {
        self.serve_static("wrpc:keyvalue/eventual@0.1.0", "delete")
            .await
    }

    #[instrument(level = "trace", skip_all)]
    async fn invoke_exists(
        &self,
        bucket: String,
        key: String,
    ) -> anyhow::Result<(Result<bool, String>, T::Transmission)> {
        self.invoke_static("wrpc:keyvalue/eventual@0.1.0", "exists", (bucket, key))
            .await
    }
    #[instrument(level = "trace", skip_all)]
    async fn serve_exists(&self) -> anyhow::Result<Self::ExistsInvocationStream> {
        self.serve_static("wrpc:keyvalue/eventual@0.1.0", "exists")
            .await
    }

    #[instrument(level = "trace", skip_all)]
    async fn invoke_get(
        &self,
        bucket: String,
        key: String,
    ) -> anyhow::Result<(
        Result<Option<IncomingInputStream>, String>,
        Self::Transmission,
    )> {
        self.invoke_static("wrpc:keyvalue/eventual@0.1.0", "get", (bucket, key))
            .await
    }
    #[instrument(level = "trace", skip_all)]
    async fn serve_get(&self) -> anyhow::Result<Self::GetInvocationStream> {
        self.serve_static("wrpc:keyvalue/eventual@0.1.0", "get")
            .await
    }

    #[instrument(level = "trace", skip_all)]
    async fn invoke_set(
        &self,
        bucket: String,
        key: String,
        value: impl Stream<Item = Bytes> + Send + 'static,
    ) -> anyhow::Result<(Result<(), String>, Self::Transmission)> {
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
        .await
    }
    #[instrument(level = "trace", skip_all)]
    async fn serve_set(&self) -> anyhow::Result<Self::SetInvocationStream> {
        self.serve_static("wrpc:keyvalue/eventual@0.1.0", "set")
            .await
    }
}

pub trait Atomic: wrpc_transport::Client {
    type CompareAndSwapInvocationStream;
    type IncrementInvocationStream;

    fn invoke_compare_and_swap(
        &self,
        bucket: String,
        key: String,
        old: u64,
        new: u64,
    ) -> impl Future<Output = anyhow::Result<(Result<bool, String>, Self::Transmission)>> + Send;
    fn serve_compare_and_swap(
        &self,
    ) -> impl Future<Output = anyhow::Result<Self::CompareAndSwapInvocationStream>> + Send;

    fn invoke_increment(
        &self,
        bucket: String,
        key: String,
        delta: u64,
    ) -> impl Future<Output = anyhow::Result<(Result<u64, String>, Self::Transmission)>> + Send;
    fn serve_increment(
        &self,
    ) -> impl Future<Output = anyhow::Result<Self::IncrementInvocationStream>> + Send;
}

impl<T: wrpc_transport::Client> Atomic for T {
    type CompareAndSwapInvocationStream = Pin<
        Box<
            dyn Stream<
                    Item = anyhow::Result<
                        AcceptedInvocation<
                            T::Context,
                            (String, String, u64, u64),
                            T::Subject,
                            <T::Acceptor as Acceptor>::Transmitter,
                        >,
                    >,
                > + Send,
        >,
    >;

    type IncrementInvocationStream = Pin<
        Box<
            dyn Stream<
                    Item = anyhow::Result<
                        AcceptedInvocation<
                            T::Context,
                            (String, String, u64),
                            T::Subject,
                            <T::Acceptor as Acceptor>::Transmitter,
                        >,
                    >,
                > + Send,
        >,
    >;

    #[instrument(level = "trace", skip_all)]
    async fn invoke_compare_and_swap(
        &self,
        bucket: String,
        key: String,
        old: u64,
        new: u64,
    ) -> anyhow::Result<(Result<bool, String>, Self::Transmission)> {
        self.invoke_static(
            "wrpc:keyvalue/atomic@0.1.0",
            "compare-and-swap",
            (bucket, key, old, new),
        )
        .await
    }
    #[instrument(level = "trace", skip_all)]
    async fn serve_compare_and_swap(&self) -> anyhow::Result<Self::CompareAndSwapInvocationStream> {
        self.serve_static("wrpc:keyvalue/atomic@0.1.0", "compare-and-swap")
            .await
    }

    #[instrument(level = "trace", skip_all)]
    async fn invoke_increment(
        &self,
        bucket: String,
        key: String,
        delta: u64,
    ) -> anyhow::Result<(Result<u64, String>, Self::Transmission)> {
        self.invoke_static(
            "wrpc:keyvalue/atomic@0.1.0",
            "increment",
            (bucket, key, delta),
        )
        .await
    }
    #[instrument(level = "trace", skip_all)]
    async fn serve_increment(&self) -> anyhow::Result<Self::IncrementInvocationStream> {
        self.serve_static("wrpc:keyvalue/atomic@0.1.0", "increment")
            .await
    }
}
