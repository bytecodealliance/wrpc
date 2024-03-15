use core::future::Future;

use bytes::Bytes;
use futures::{Stream, StreamExt as _};
use tracing::instrument;
use wrpc_transport::{IncomingInputStream, Value};

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
    ) -> impl Future<Output = anyhow::Result<Self::InvocationStream<(String, String)>>> + Send {
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
    ) -> impl Future<Output = anyhow::Result<Self::InvocationStream<(String, String)>>> + Send {
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
    ) -> impl Future<Output = anyhow::Result<Self::InvocationStream<(String, String)>>> + Send {
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
        Output = anyhow::Result<Self::InvocationStream<(String, String, IncomingInputStream)>>,
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
    ) -> impl Future<Output = anyhow::Result<Self::InvocationStream<(String, String, u64, u64)>>> + Send
    {
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
    ) -> impl Future<Output = anyhow::Result<Self::InvocationStream<(String, String, u64)>>> + Send
    {
        self.serve_static("wrpc:keyvalue/atomic@0.1.0", "increment")
    }
}

impl<T: wrpc_transport::Client> Atomic for T {}
