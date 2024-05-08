#![allow(clippy::type_complexity)] // TODO: https://github.com/wrpc/wrpc/issues/2

use core::future::Future;

use anyhow::Context as _;
use async_trait::async_trait;
use bytes::{Buf, BufMut, Bytes};
use futures::{try_join, Stream, StreamExt as _};
use tracing::instrument;
use wrpc_transport::{
    Acceptor, AsyncSubscription, AsyncValue, Encode, IncomingInputStream, ListIter, Receive,
    Subscribe, Value,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContainerMetadata {
    pub created_at: u64,
}

impl Subscribe for ContainerMetadata {}

#[async_trait]
impl Encode for ContainerMetadata {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        let Self { created_at } = self;
        created_at
            .encode(payload)
            .await
            .context("failed to encode `created-at`")?;
        Ok(None)
    }
}

#[async_trait]
impl<'a> Receive<'a> for ContainerMetadata {
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
    {
        let (created_at, payload) = Receive::receive_sync(payload, rx)
            .await
            .context("failed to receive `created-at`")?;
        Ok((Self { created_at }, payload))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectMetadata {
    pub created_at: u64,
    pub size: u64,
}

impl Subscribe for ObjectMetadata {}

#[async_trait]
impl Encode for ObjectMetadata {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        mut payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        let Self { created_at, size } = self;
        created_at
            .encode(&mut payload)
            .await
            .context("failed to encode `created-at`")?;
        size.encode(payload)
            .await
            .context("failed to encode `size`")?;
        Ok(None)
    }
}

#[async_trait]
impl<'a> Receive<'a> for ObjectMetadata {
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
    {
        let (created_at, payload) = Receive::receive_sync(payload, rx)
            .await
            .context("failed to receive `created-at`")?;
        let (size, payload) = Receive::receive_sync(payload, rx)
            .await
            .context("failed to receive `size`")?;
        Ok((Self { created_at, size }, payload))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectId {
    pub container: String,
    pub object: String,
}

impl Subscribe for ObjectId {}

#[async_trait]
impl Encode for &ObjectId {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        let ObjectId { container, object } = self;
        container
            .as_str()
            .encode(payload)
            .await
            .context("failed to encode `container`")?;
        object
            .as_str()
            .encode(payload)
            .await
            .context("failed to encode `object`")?;
        Ok(None)
    }
}

#[async_trait]
impl Encode for ObjectId {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        (&self).encode(payload).await
    }
}

#[async_trait]
impl<'a> Receive<'a> for ObjectId {
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
    {
        let (container, payload) = Receive::receive_sync(payload, rx)
            .await
            .context("failed to receive `container`")?;
        let (object, payload) = Receive::receive_sync(payload, rx)
            .await
            .context("failed to receive `object`")?;
        Ok((Self { container, object }, payload))
    }
}

pub trait Blobstore: wrpc_transport::Client {
    #[instrument(level = "trace", skip_all)]
    fn invoke_clear_container(
        &self,
        name: &str,
    ) -> impl Future<Output = anyhow::Result<(Result<(), String>, Self::Transmission)>> + Send {
        self.invoke_static("wrpc:blobstore/blobstore@0.1.0", "clear-container", name)
    }

    #[instrument(level = "trace", skip_all)]
    fn serve_clear_container(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            Self::InvocationStream<
                Self::Context,
                String,
                <Self::Acceptor as Acceptor>::Transmitter,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "clear-container")
    }

    #[instrument(level = "trace", skip_all)]
    fn invoke_container_exists(
        &self,
        name: &str,
    ) -> impl Future<Output = anyhow::Result<(Result<bool, String>, Self::Transmission)>> + Send
    {
        self.invoke_static("wrpc:blobstore/blobstore@0.1.0", "container-exists", name)
    }

    #[instrument(level = "trace", skip_all)]
    fn serve_container_exists(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            Self::InvocationStream<
                Self::Context,
                String,
                <Self::Acceptor as Acceptor>::Transmitter,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "container-exists")
    }

    #[instrument(level = "trace", skip_all)]
    fn invoke_create_container(
        &self,
        name: &str,
    ) -> impl Future<Output = anyhow::Result<(Result<(), String>, Self::Transmission)>> + Send {
        self.invoke_static("wrpc:blobstore/blobstore@0.1.0", "create-container", name)
    }

    #[instrument(level = "trace", skip_all)]
    fn serve_create_container(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            Self::InvocationStream<
                Self::Context,
                String,
                <Self::Acceptor as Acceptor>::Transmitter,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "create-container")
    }

    #[instrument(level = "trace", skip_all)]
    fn invoke_delete_container(
        &self,
        name: &str,
    ) -> impl Future<Output = anyhow::Result<(Result<(), String>, Self::Transmission)>> + Send {
        self.invoke_static("wrpc:blobstore/blobstore@0.1.0", "delete-container", name)
    }

    #[instrument(level = "trace", skip_all)]
    fn serve_delete_container(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            Self::InvocationStream<
                Self::Context,
                String,
                <Self::Acceptor as Acceptor>::Transmitter,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "delete-container")
    }

    #[instrument(level = "trace", skip_all)]
    fn invoke_get_container_info(
        &self,
        name: &str,
    ) -> impl Future<Output = anyhow::Result<(Result<ContainerMetadata, String>, Self::Transmission)>>
           + Send {
        self.invoke_static("wrpc:blobstore/blobstore@0.1.0", "get-container-info", name)
    }
    #[instrument(level = "trace", skip_all)]
    fn serve_get_container_info(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            Self::InvocationStream<
                Self::Context,
                String,
                <Self::Acceptor as Acceptor>::Transmitter,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "get-container-info")
    }

    #[instrument(level = "trace", skip_all)]
    fn invoke_list_container_objects(
        &self,
        name: &str,
        limit: Option<u64>,
        offset: Option<u64>,
    ) -> impl Future<
        Output = anyhow::Result<(
            Result<
                Box<dyn Stream<Item = anyhow::Result<Vec<String>>> + Send + Sync + Unpin>,
                String,
            >,
            Self::Transmission,
        )>,
    > + Send {
        self.invoke_static(
            "wrpc:blobstore/blobstore@0.1.0",
            "list-container-objects",
            (name, limit, offset),
        )
    }
    #[instrument(level = "trace", skip_all)]
    fn serve_list_container_objects(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            Self::InvocationStream<
                Self::Context,
                (String, Option<u64>, Option<u64>),
                <Self::Acceptor as Acceptor>::Transmitter,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "list-container-objects")
    }

    #[instrument(level = "trace", skip_all)]
    fn invoke_copy_object(
        &self,
        src: &ObjectId,
        dest: &ObjectId,
    ) -> impl Future<Output = anyhow::Result<(Result<(), String>, Self::Transmission)>> + Send {
        self.invoke_static("wrpc:blobstore/blobstore@0.1.0", "copy-object", (src, dest))
    }

    #[instrument(level = "trace", skip_all)]
    fn serve_copy_object(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            Self::InvocationStream<
                Self::Context,
                (ObjectId, ObjectId),
                <Self::Acceptor as Acceptor>::Transmitter,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "copy-object")
    }

    #[instrument(level = "trace", skip_all)]
    fn invoke_delete_object(
        &self,
        id: &ObjectId,
    ) -> impl Future<Output = anyhow::Result<(Result<(), String>, Self::Transmission)>> + Send {
        self.invoke_static("wrpc:blobstore/blobstore@0.1.0", "delete-object", id)
    }
    #[instrument(level = "trace", skip_all)]
    fn serve_delete_object(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            Self::InvocationStream<
                Self::Context,
                ObjectId,
                <Self::Acceptor as Acceptor>::Transmitter,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "delete-object")
    }

    #[instrument(level = "trace", skip_all)]
    fn invoke_delete_objects<'a, I>(
        &self,
        container: &str,
        objects: I,
    ) -> impl Future<Output = anyhow::Result<(Result<(), String>, Self::Transmission)>> + Send
    where
        I: IntoIterator<Item = &'a str> + Send,
        I::IntoIter: ExactSizeIterator + Send,
    {
        self.invoke_static(
            "wrpc:blobstore/blobstore@0.1.0",
            "delete-objects",
            (container, ListIter(objects)),
        )
    }

    #[instrument(level = "trace", skip_all)]
    fn serve_delete_objects(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            Self::InvocationStream<
                Self::Context,
                (String, Vec<String>),
                <Self::Acceptor as Acceptor>::Transmitter,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "delete-objects")
    }

    #[instrument(level = "trace", skip_all)]
    fn invoke_get_container_data(
        &self,
        id: &ObjectId,
        start: u64,
        end: u64,
    ) -> impl Future<
        Output = anyhow::Result<(Result<IncomingInputStream, String>, Self::Transmission)>,
    > + Send {
        self.invoke_static(
            "wrpc:blobstore/blobstore@0.1.0",
            "get-container-data",
            (id, start, end),
        )
    }

    #[instrument(level = "trace", skip_all)]
    fn serve_get_container_data(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            Self::InvocationStream<
                Self::Context,
                (ObjectId, u64, u64),
                <Self::Acceptor as Acceptor>::Transmitter,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "get-container-data")
    }

    #[instrument(level = "trace", skip_all)]
    fn invoke_get_object_info(
        &self,
        id: &ObjectId,
    ) -> impl Future<Output = anyhow::Result<(Result<ObjectMetadata, String>, Self::Transmission)>> + Send
    {
        self.invoke_static("wrpc:blobstore/blobstore@0.1.0", "get-object-info", id)
    }

    #[instrument(level = "trace", skip_all)]
    fn serve_get_object_info(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            Self::InvocationStream<
                Self::Context,
                ObjectId,
                <Self::Acceptor as Acceptor>::Transmitter,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "get-object-info")
    }

    #[instrument(level = "trace", skip_all)]
    fn invoke_has_object(
        &self,
        id: &ObjectId,
    ) -> impl Future<Output = anyhow::Result<(Result<bool, String>, Self::Transmission)>> + Send
    {
        self.invoke_static("wrpc:blobstore/blobstore@0.1.0", "has-object", id)
    }

    #[instrument(level = "trace", skip_all)]
    fn serve_has_object(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            Self::InvocationStream<
                Self::Context,
                ObjectId,
                <Self::Acceptor as Acceptor>::Transmitter,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "has-object")
    }

    #[instrument(level = "trace", skip_all)]
    fn invoke_move_object(
        &self,
        src: &ObjectId,
        dest: &ObjectId,
    ) -> impl Future<Output = anyhow::Result<(Result<(), String>, Self::Transmission)>> + Send {
        self.invoke_static("wrpc:blobstore/blobstore@0.1.0", "move-object", (src, dest))
    }

    #[instrument(level = "trace", skip_all)]
    fn serve_move_object(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            Self::InvocationStream<
                Self::Context,
                (ObjectId, ObjectId),
                <Self::Acceptor as Acceptor>::Transmitter,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "move-object")
    }

    #[instrument(level = "trace", skip_all)]
    fn invoke_write_container_data(
        &self,
        id: &ObjectId,
        data: impl Stream<Item = Bytes> + Send + 'static,
    ) -> impl Future<Output = anyhow::Result<(Result<(), String>, Self::Transmission)>> + Send {
        self.invoke_static(
            "wrpc:blobstore/blobstore@0.1.0",
            "write-container-data",
            (
                id,
                Value::Stream(Box::pin(
                    data.map(|buf| Ok(buf.into_iter().map(Value::U8).map(Some).collect())),
                )),
            ),
        )
    }

    #[instrument(level = "trace", skip_all)]
    fn serve_write_container_data(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            Self::InvocationStream<
                Self::Context,
                (ObjectId, IncomingInputStream),
                <Self::Acceptor as Acceptor>::Transmitter,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "write-container-data")
    }
}

impl<T: wrpc_transport::Client> Blobstore for T {}

/// `wrpc:blobstore/blobstore` invocation streams
pub struct BlobstoreInvocations<T>
where
    T: wrpc_transport::Client,
{
    pub clear_container:
        T::InvocationStream<T::Context, String, <T::Acceptor as Acceptor>::Transmitter>,
    pub container_exists:
        T::InvocationStream<T::Context, String, <T::Acceptor as Acceptor>::Transmitter>,
    pub create_container:
        T::InvocationStream<T::Context, String, <T::Acceptor as Acceptor>::Transmitter>,
    pub delete_container:
        T::InvocationStream<T::Context, String, <T::Acceptor as Acceptor>::Transmitter>,
    pub get_container_info:
        T::InvocationStream<T::Context, String, <T::Acceptor as Acceptor>::Transmitter>,
    pub list_container_objects: T::InvocationStream<
        T::Context,
        (String, Option<u64>, Option<u64>),
        <T::Acceptor as Acceptor>::Transmitter,
    >,
    pub copy_object: T::InvocationStream<
        T::Context,
        (ObjectId, ObjectId),
        <T::Acceptor as Acceptor>::Transmitter,
    >,
    pub delete_object:
        T::InvocationStream<T::Context, ObjectId, <T::Acceptor as Acceptor>::Transmitter>,
    pub delete_objects: T::InvocationStream<
        T::Context,
        (String, Vec<String>),
        <T::Acceptor as Acceptor>::Transmitter,
    >,
    pub get_container_data: T::InvocationStream<
        T::Context,
        (ObjectId, u64, u64),
        <T::Acceptor as Acceptor>::Transmitter,
    >,
    pub get_object_info:
        T::InvocationStream<T::Context, ObjectId, <T::Acceptor as Acceptor>::Transmitter>,
    pub has_object:
        T::InvocationStream<T::Context, ObjectId, <T::Acceptor as Acceptor>::Transmitter>,
    pub move_object: T::InvocationStream<
        T::Context,
        (ObjectId, ObjectId),
        <T::Acceptor as Acceptor>::Transmitter,
    >,
    pub write_container_data: T::InvocationStream<
        T::Context,
        (ObjectId, IncomingInputStream),
        <T::Acceptor as Acceptor>::Transmitter,
    >,
}

/// Serve `wrpc:blobstore/blobstore` invocations
#[instrument(level = "trace", skip_all)]
pub async fn serve_blobstore<T>(client: &T) -> anyhow::Result<BlobstoreInvocations<T>>
where
    T: Blobstore,
{
    let (
        clear_container,
        container_exists,
        create_container,
        delete_container,
        get_container_info,
        list_container_objects,
        copy_object,
        delete_object,
        delete_objects,
        get_container_data,
        get_object_info,
        has_object,
        move_object,
        write_container_data,
    ) = try_join!(
        async {
            client
                .serve_clear_container()
                .await
                .context("failed to serve `wrpc:blobstore/blobstore.clear-container`")
        },
        async {
            client
                .serve_container_exists()
                .await
                .context("failed to serve `wrpc:blobstore/blobstore.container-exists`")
        },
        async {
            client
                .serve_create_container()
                .await
                .context("failed to serve `wrpc:blobstore/blobstore.create-container`")
        },
        async {
            client
                .serve_delete_container()
                .await
                .context("failed to serve `wrpc:blobstore/blobstore.delete-container`")
        },
        async {
            client
                .serve_get_container_info()
                .await
                .context("failed to serve `wrpc:blobstore/blobstore.get-container-info`")
        },
        async {
            client
                .serve_list_container_objects()
                .await
                .context("failed to serve `wrpc:blobstore/blobstore.list-container-objects`")
        },
        async {
            client
                .serve_copy_object()
                .await
                .context("failed to serve `wrpc:blobstore/blobstore.copy-object`")
        },
        async {
            client
                .serve_delete_object()
                .await
                .context("failed to serve `wrpc:blobstore/blobstore.delete-object`")
        },
        async {
            client
                .serve_delete_objects()
                .await
                .context("failed to serve `wrpc:blobstore/blobstore.delete-objects`")
        },
        async {
            client
                .serve_get_container_data()
                .await
                .context("failed to serve `wrpc:blobstore/blobstore.get-container-data`")
        },
        async {
            client
                .serve_get_object_info()
                .await
                .context("failed to serve `wrpc:blobstore/blobstore.get-object-info`")
        },
        async {
            client
                .serve_has_object()
                .await
                .context("failed to serve `wrpc:blobstore/blobstore.has-object`")
        },
        async {
            client
                .serve_move_object()
                .await
                .context("failed to serve `wrpc:blobstore/blobstore.move-object`")
        },
        async {
            client
                .serve_write_container_data()
                .await
                .context("failed to serve `wrpc:blobstore/blobstore.write-container-data`")
        },
    )?;
    Ok(BlobstoreInvocations {
        clear_container,
        container_exists,
        create_container,
        delete_container,
        get_container_info,
        list_container_objects,
        copy_object,
        delete_object,
        delete_objects,
        get_container_data,
        get_object_info,
        has_object,
        move_object,
        write_container_data,
    })
}
