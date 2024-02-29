use core::future::Future;
use core::pin::Pin;

use anyhow::Context as _;
use async_trait::async_trait;
use bytes::{Buf, BufMut, Bytes};
use futures::{Stream, StreamExt as _};
use tracing::instrument;
use wrpc_transport::{
    AcceptedInvocation, Acceptor, AsyncSubscription, EncodeSync, IncomingInputStream, Receive,
    Value,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContainerMetadata {
    pub name: String,
    pub created_at: u64,
}

impl EncodeSync for ContainerMetadata {
    #[instrument(level = "trace", skip_all)]
    fn encode_sync(self, mut payload: impl BufMut) -> anyhow::Result<()> {
        let Self { name, created_at } = self;
        name.encode_sync(&mut payload)
            .context("failed to encode `name`")?;
        created_at
            .encode_sync(payload)
            .context("failed to encode `created-at`")?;
        Ok(())
    }
}

#[async_trait]
impl Receive for ContainerMetadata {
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        let (name, payload) = Receive::receive_sync(payload, rx)
            .await
            .context("failed to receive `name`")?;
        let (created_at, payload) = Receive::receive_sync(payload, rx)
            .await
            .context("failed to receive `created-at`")?;
        Ok((Self { name, created_at }, payload))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectMetadata {
    pub name: String,
    pub container: String,
    pub created_at: u64,
    pub size: u64,
}

impl EncodeSync for ObjectMetadata {
    #[instrument(level = "trace", skip_all)]
    fn encode_sync(self, mut payload: impl BufMut) -> anyhow::Result<()> {
        let Self {
            name,
            container,
            created_at,
            size,
        } = self;
        name.encode_sync(&mut payload)
            .context("failed to encode `name`")?;
        container
            .encode_sync(&mut payload)
            .context("failed to encode `container`")?;
        created_at
            .encode_sync(&mut payload)
            .context("failed to encode `created-at`")?;
        size.encode_sync(payload)
            .context("failed to encode `size`")?;
        Ok(())
    }
}

#[async_trait]
impl Receive for ObjectMetadata {
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        let (name, payload) = Receive::receive_sync(payload, rx)
            .await
            .context("failed to receive `name`")?;
        let (container, payload) = Receive::receive_sync(payload, rx)
            .await
            .context("failed to receive `container`")?;
        let (created_at, payload) = Receive::receive_sync(payload, rx)
            .await
            .context("failed to receive `created-at`")?;
        let (size, payload) = Receive::receive_sync(payload, rx)
            .await
            .context("failed to receive `size`")?;
        Ok((
            Self {
                name,
                container,
                created_at,
                size,
            },
            payload,
        ))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectId {
    pub container: String,
    pub object: String,
}

impl EncodeSync for ObjectId {
    #[instrument(level = "trace", skip_all)]
    fn encode_sync(self, mut payload: impl BufMut) -> anyhow::Result<()> {
        let Self { container, object } = self;
        container
            .encode_sync(&mut payload)
            .context("failed to encode `container`")?;
        object
            .encode_sync(payload)
            .context("failed to encode `object`")?;
        Ok(())
    }
}

#[async_trait]
impl Receive for ObjectId {
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
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

type StringInvocationStream<T> = Pin<
    Box<
        dyn Stream<
                Item = anyhow::Result<
                    AcceptedInvocation<
                        <T as wrpc_transport::Client>::Context,
                        String,
                        <T as wrpc_transport::Client>::Subject,
                        <<T as wrpc_transport::Client>::Acceptor as Acceptor>::Transmitter,
                    >,
                >,
            > + Send,
    >,
>;

type ObjectIdInvocationStream<T> = Pin<
    Box<
        dyn Stream<
                Item = anyhow::Result<
                    AcceptedInvocation<
                        <T as wrpc_transport::Client>::Context,
                        ObjectId,
                        <T as wrpc_transport::Client>::Subject,
                        <<T as wrpc_transport::Client>::Acceptor as Acceptor>::Transmitter,
                    >,
                >,
            > + Send,
    >,
>;

pub trait Blobstore: wrpc_transport::Client {
    type ClearContainerInvocationStream;
    type ContainerExistsInvocationStream;
    type CreateContainerInvocationStream;
    type DeleteContainerInvocationStream;
    type GetContainerInfoInvocationStream;
    type ListContainerObjectsInvocationStream;
    type CopyObjectInvocationStream;
    type DeleteObjectInvocationStream;
    type DeleteObjectsInvocationStream;
    type GetContainerDataInvocationStream;
    type GetObjectInfoInvocationStream;
    type HasObjectInvocationStream;
    type MoveObjectInvocationStream;
    type WriteContainerDataInvocationStream;

    fn invoke_clear_container(
        &self,
        name: String,
    ) -> impl Future<Output = anyhow::Result<(Result<(), String>, Self::Transmission)>> + Send;
    fn serve_clear_container(
        &self,
    ) -> impl Future<Output = anyhow::Result<Self::ClearContainerInvocationStream>> + Send;

    fn invoke_container_exists(
        &self,
        name: String,
    ) -> impl Future<Output = anyhow::Result<(Result<bool, String>, Self::Transmission)>> + Send;
    fn serve_container_exists(
        &self,
    ) -> impl Future<Output = anyhow::Result<Self::ContainerExistsInvocationStream>> + Send;

    fn invoke_create_container(
        &self,
        name: String,
    ) -> impl Future<Output = anyhow::Result<(Result<(), String>, Self::Transmission)>> + Send;
    fn serve_create_container(
        &self,
    ) -> impl Future<Output = anyhow::Result<Self::CreateContainerInvocationStream>> + Send;

    fn invoke_delete_container(
        &self,
        name: String,
    ) -> impl Future<Output = anyhow::Result<(Result<(), String>, Self::Transmission)>> + Send;
    fn serve_delete_container(
        &self,
    ) -> impl Future<Output = anyhow::Result<Self::DeleteContainerInvocationStream>> + Send;

    fn invoke_get_container_info(
        &self,
        name: String,
    ) -> impl Future<Output = anyhow::Result<(Result<ContainerMetadata, String>, Self::Transmission)>>
           + Send;
    fn serve_get_container_info(
        &self,
    ) -> impl Future<Output = anyhow::Result<Self::GetContainerInfoInvocationStream>> + Send;

    fn invoke_list_container_objects(
        &self,
        name: String,
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
    > + Send;
    fn serve_list_container_objects(
        &self,
    ) -> impl Future<Output = anyhow::Result<Self::ListContainerObjectsInvocationStream>> + Send;

    fn invoke_copy_object(
        &self,
        src: ObjectId,
        dest: ObjectId,
    ) -> impl Future<Output = anyhow::Result<(Result<(), String>, Self::Transmission)>> + Send;
    fn serve_copy_object(
        &self,
    ) -> impl Future<Output = anyhow::Result<Self::CopyObjectInvocationStream>> + Send;

    fn invoke_delete_object(
        &self,
        id: ObjectId,
    ) -> impl Future<Output = anyhow::Result<(Result<(), String>, Self::Transmission)>> + Send;
    fn serve_delete_object(
        &self,
    ) -> impl Future<Output = anyhow::Result<Self::DeleteObjectInvocationStream>> + Send;

    fn invoke_delete_objects(
        &self,
        container: String,
        objects: Vec<String>,
    ) -> impl Future<Output = anyhow::Result<(Result<(), String>, Self::Transmission)>> + Send;
    fn serve_delete_objects(
        &self,
    ) -> impl Future<Output = anyhow::Result<Self::DeleteObjectsInvocationStream>> + Send;

    fn invoke_get_container_data(
        &self,
        id: ObjectId,
        start: u64,
        end: u64,
    ) -> impl Future<
        Output = anyhow::Result<(Result<IncomingInputStream, String>, Self::Transmission)>,
    > + Send;
    fn serve_get_container_data(
        &self,
    ) -> impl Future<Output = anyhow::Result<Self::GetContainerDataInvocationStream>> + Send;

    fn invoke_get_object_info(
        &self,
        id: ObjectId,
    ) -> impl Future<Output = anyhow::Result<(Result<ObjectMetadata, String>, Self::Transmission)>> + Send;
    fn serve_get_object_info(
        &self,
    ) -> impl Future<Output = anyhow::Result<Self::GetObjectInfoInvocationStream>> + Send;

    fn invoke_has_object(
        &self,
        id: ObjectId,
    ) -> impl Future<Output = anyhow::Result<(Result<bool, String>, Self::Transmission)>> + Send;
    fn serve_has_object(
        &self,
    ) -> impl Future<Output = anyhow::Result<Self::HasObjectInvocationStream>> + Send;

    fn invoke_move_object(
        &self,
        src: ObjectId,
        dest: ObjectId,
    ) -> impl Future<Output = anyhow::Result<(Result<(), String>, Self::Transmission)>> + Send;
    fn serve_move_object(
        &self,
    ) -> impl Future<Output = anyhow::Result<Self::MoveObjectInvocationStream>> + Send;

    fn invoke_write_container_data(
        &self,
        id: ObjectId,
        data: impl Stream<Item = Bytes> + Send + 'static,
    ) -> impl Future<Output = anyhow::Result<(Result<(), String>, Self::Transmission)>> + Send;
    fn serve_write_container_data(
        &self,
    ) -> impl Future<Output = anyhow::Result<Self::WriteContainerDataInvocationStream>> + Send;
}

impl<T: wrpc_transport::Client> Blobstore for T {
    type ClearContainerInvocationStream = StringInvocationStream<T>;
    type ContainerExistsInvocationStream = StringInvocationStream<T>;
    type CreateContainerInvocationStream = StringInvocationStream<T>;
    type DeleteContainerInvocationStream = StringInvocationStream<T>;
    type GetContainerInfoInvocationStream = StringInvocationStream<T>;
    type ListContainerObjectsInvocationStream = Pin<
        Box<
            dyn Stream<
                    Item = anyhow::Result<
                        AcceptedInvocation<
                            T::Context,
                            (String, Option<u64>, Option<u64>),
                            T::Subject,
                            <T::Acceptor as Acceptor>::Transmitter,
                        >,
                    >,
                > + Send,
        >,
    >;
    type CopyObjectInvocationStream = Pin<
        Box<
            dyn Stream<
                    Item = anyhow::Result<
                        AcceptedInvocation<
                            T::Context,
                            (ObjectId, ObjectId),
                            T::Subject,
                            <T::Acceptor as Acceptor>::Transmitter,
                        >,
                    >,
                > + Send,
        >,
    >;
    type DeleteObjectInvocationStream = ObjectIdInvocationStream<T>;
    type DeleteObjectsInvocationStream = Pin<
        Box<
            dyn Stream<
                    Item = anyhow::Result<
                        AcceptedInvocation<
                            T::Context,
                            (String, Vec<String>),
                            T::Subject,
                            <T::Acceptor as Acceptor>::Transmitter,
                        >,
                    >,
                > + Send,
        >,
    >;
    type GetContainerDataInvocationStream = Pin<
        Box<
            dyn Stream<
                    Item = anyhow::Result<
                        AcceptedInvocation<
                            T::Context,
                            (ObjectId, u64, u64),
                            T::Subject,
                            <T::Acceptor as Acceptor>::Transmitter,
                        >,
                    >,
                > + Send,
        >,
    >;
    type GetObjectInfoInvocationStream = ObjectIdInvocationStream<T>;
    type HasObjectInvocationStream = ObjectIdInvocationStream<T>;
    type MoveObjectInvocationStream = Pin<
        Box<
            dyn Stream<
                    Item = anyhow::Result<
                        AcceptedInvocation<
                            T::Context,
                            (ObjectId, ObjectId),
                            T::Subject,
                            <T::Acceptor as Acceptor>::Transmitter,
                        >,
                    >,
                > + Send,
        >,
    >;
    type WriteContainerDataInvocationStream = Pin<
        Box<
            dyn Stream<
                    Item = anyhow::Result<
                        AcceptedInvocation<
                            T::Context,
                            (ObjectId, IncomingInputStream),
                            T::Subject,
                            <T::Acceptor as Acceptor>::Transmitter,
                        >,
                    >,
                > + Send,
        >,
    >;

    async fn invoke_clear_container(
        &self,
        name: String,
    ) -> anyhow::Result<(Result<(), String>, T::Transmission)> {
        self.invoke_static("wrpc:blobstore/blobstore@0.1.0", "clear-container", name)
            .await
    }
    async fn serve_clear_container(&self) -> anyhow::Result<Self::ClearContainerInvocationStream> {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "clear-container")
            .await
    }

    async fn invoke_container_exists(
        &self,
        name: String,
    ) -> anyhow::Result<(Result<bool, String>, T::Transmission)> {
        self.invoke_static("wrpc:blobstore/blobstore@0.1.0", "container-exists", name)
            .await
    }
    async fn serve_container_exists(
        &self,
    ) -> anyhow::Result<Self::ContainerExistsInvocationStream> {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "container-exists")
            .await
    }

    async fn invoke_create_container(
        &self,
        name: String,
    ) -> anyhow::Result<(Result<(), String>, T::Transmission)> {
        self.invoke_static("wrpc:blobstore/blobstore@0.1.0", "create-container", name)
            .await
    }
    async fn serve_create_container(
        &self,
    ) -> anyhow::Result<Self::CreateContainerInvocationStream> {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "create-container")
            .await
    }

    async fn invoke_delete_container(
        &self,
        name: String,
    ) -> anyhow::Result<(Result<(), String>, T::Transmission)> {
        self.invoke_static("wrpc:blobstore/blobstore@0.1.0", "delete-container", name)
            .await
    }
    async fn serve_delete_container(
        &self,
    ) -> anyhow::Result<Self::DeleteContainerInvocationStream> {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "delete-container")
            .await
    }

    async fn invoke_get_container_info(
        &self,
        name: String,
    ) -> anyhow::Result<(Result<ContainerMetadata, String>, T::Transmission)> {
        self.invoke_static("wrpc:blobstore/blobstore@0.1.0", "get-container-info", name)
            .await
    }
    async fn serve_get_container_info(
        &self,
    ) -> anyhow::Result<Self::GetContainerInfoInvocationStream> {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "get-container-info")
            .await
    }

    async fn invoke_list_container_objects(
        &self,
        name: String,
        limit: Option<u64>,
        offset: Option<u64>,
    ) -> anyhow::Result<(
        Result<Box<dyn Stream<Item = anyhow::Result<Vec<String>>> + Send + Sync + Unpin>, String>,
        Self::Transmission,
    )> {
        self.invoke_static(
            "wrpc:blobstore/blobstore@0.1.0",
            "list-container-objects",
            (name, limit, offset),
        )
        .await
    }
    async fn serve_list_container_objects(
        &self,
    ) -> anyhow::Result<Self::ListContainerObjectsInvocationStream> {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "list-container-objects")
            .await
    }

    async fn invoke_copy_object(
        &self,
        src: ObjectId,
        dest: ObjectId,
    ) -> anyhow::Result<(Result<(), String>, Self::Transmission)> {
        self.invoke_static("wrpc:blobstore/blobstore@0.1.0", "copy-object", (src, dest))
            .await
    }
    async fn serve_copy_object(&self) -> anyhow::Result<Self::CopyObjectInvocationStream> {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "copy-object")
            .await
    }

    async fn invoke_delete_object(
        &self,
        id: ObjectId,
    ) -> anyhow::Result<(Result<(), String>, Self::Transmission)> {
        self.invoke_static("wrpc:blobstore/blobstore@0.1.0", "delete-object", id)
            .await
    }
    async fn serve_delete_object(&self) -> anyhow::Result<Self::DeleteObjectInvocationStream> {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "delete-object")
            .await
    }

    async fn invoke_delete_objects(
        &self,
        container: String,
        objects: Vec<String>,
    ) -> anyhow::Result<(Result<(), String>, Self::Transmission)> {
        self.invoke_static(
            "wrpc:blobstore/blobstore@0.1.0",
            "delete-objects",
            (container, objects),
        )
        .await
    }
    async fn serve_delete_objects(&self) -> anyhow::Result<Self::DeleteObjectsInvocationStream> {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "delete-objects")
            .await
    }

    async fn invoke_get_container_data(
        &self,
        id: ObjectId,
        start: u64,
        end: u64,
    ) -> anyhow::Result<(Result<IncomingInputStream, String>, Self::Transmission)> {
        self.invoke_static(
            "wrpc:blobstore/blobstore@0.1.0",
            "get-container-data",
            (id, start, end),
        )
        .await
    }
    async fn serve_get_container_data(
        &self,
    ) -> anyhow::Result<Self::GetContainerDataInvocationStream> {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "get-container-data")
            .await
    }

    async fn invoke_get_object_info(
        &self,
        id: ObjectId,
    ) -> anyhow::Result<(Result<ObjectMetadata, String>, Self::Transmission)> {
        self.invoke_static("wrpc:blobstore/blobstore@0.1.0", "get-object-info", id)
            .await
    }
    async fn serve_get_object_info(&self) -> anyhow::Result<Self::GetObjectInfoInvocationStream> {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "get-object-info")
            .await
    }

    async fn invoke_has_object(
        &self,
        id: ObjectId,
    ) -> anyhow::Result<(Result<bool, String>, Self::Transmission)> {
        self.invoke_static("wrpc:blobstore/blobstore@0.1.0", "has-object", id)
            .await
    }
    async fn serve_has_object(&self) -> anyhow::Result<Self::HasObjectInvocationStream> {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "has-object")
            .await
    }

    async fn invoke_move_object(
        &self,
        src: ObjectId,
        dest: ObjectId,
    ) -> anyhow::Result<(Result<(), String>, Self::Transmission)> {
        self.invoke_static("wrpc:blobstore/blobstore@0.1.0", "move-object", (src, dest))
            .await
    }
    async fn serve_move_object(&self) -> anyhow::Result<Self::MoveObjectInvocationStream> {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "move-object")
            .await
    }

    async fn invoke_write_container_data(
        &self,
        id: ObjectId,
        data: impl Stream<Item = Bytes> + Send + 'static,
    ) -> anyhow::Result<(Result<(), String>, Self::Transmission)> {
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
        .await
    }
    async fn serve_write_container_data(
        &self,
    ) -> anyhow::Result<Self::WriteContainerDataInvocationStream> {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "write-container-data")
            .await
    }
}
