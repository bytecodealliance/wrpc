use core::future::Future;

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
    async fn receive<'a, T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
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
    async fn receive<'a, T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
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
    async fn receive<'a, T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
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

pub trait Blobstore: wrpc_transport::Client {
    #[instrument(level = "trace", skip_all)]
    fn invoke_clear_container(
        &self,
        name: String,
    ) -> impl Future<Output = anyhow::Result<(Result<(), String>, Self::Transmission)>> + Send {
        self.invoke_static("wrpc:blobstore/blobstore@0.1.0", "clear-container", name)
    }

    #[instrument(level = "trace", skip_all)]
    fn serve_clear_container(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            impl Stream<
                Item = anyhow::Result<
                    AcceptedInvocation<
                        <Self as wrpc_transport::Client>::Context,
                        String,
                        <Self as wrpc_transport::Client>::Subject,
                        <<Self as wrpc_transport::Client>::Acceptor as Acceptor>::Transmitter,
                    >,
                >,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "clear-container")
    }

    #[instrument(level = "trace", skip_all)]
    fn invoke_container_exists(
        &self,
        name: String,
    ) -> impl Future<Output = anyhow::Result<(Result<bool, String>, Self::Transmission)>> + Send
    {
        self.invoke_static("wrpc:blobstore/blobstore@0.1.0", "container-exists", name)
    }

    #[instrument(level = "trace", skip_all)]
    fn serve_container_exists(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            impl Stream<
                Item = anyhow::Result<
                    AcceptedInvocation<
                        <Self as wrpc_transport::Client>::Context,
                        String,
                        <Self as wrpc_transport::Client>::Subject,
                        <<Self as wrpc_transport::Client>::Acceptor as Acceptor>::Transmitter,
                    >,
                >,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "container-exists")
    }

    #[instrument(level = "trace", skip_all)]
    fn invoke_create_container(
        &self,
        name: String,
    ) -> impl Future<Output = anyhow::Result<(Result<(), String>, Self::Transmission)>> + Send {
        self.invoke_static("wrpc:blobstore/blobstore@0.1.0", "create-container", name)
    }

    #[instrument(level = "trace", skip_all)]
    fn serve_create_container(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            impl Stream<
                Item = anyhow::Result<
                    AcceptedInvocation<
                        <Self as wrpc_transport::Client>::Context,
                        String,
                        <Self as wrpc_transport::Client>::Subject,
                        <<Self as wrpc_transport::Client>::Acceptor as Acceptor>::Transmitter,
                    >,
                >,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "create-container")
    }

    #[instrument(level = "trace", skip_all)]
    fn invoke_delete_container(
        &self,
        name: String,
    ) -> impl Future<Output = anyhow::Result<(Result<(), String>, Self::Transmission)>> + Send {
        self.invoke_static("wrpc:blobstore/blobstore@0.1.0", "delete-container", name)
    }

    #[instrument(level = "trace", skip_all)]
    fn serve_delete_container(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            impl Stream<
                Item = anyhow::Result<
                    AcceptedInvocation<
                        <Self as wrpc_transport::Client>::Context,
                        String,
                        <Self as wrpc_transport::Client>::Subject,
                        <<Self as wrpc_transport::Client>::Acceptor as Acceptor>::Transmitter,
                    >,
                >,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "delete-container")
    }

    #[instrument(level = "trace", skip_all)]
    fn invoke_get_container_info(
        &self,
        name: String,
    ) -> impl Future<Output = anyhow::Result<(Result<ContainerMetadata, String>, Self::Transmission)>>
           + Send {
        self.invoke_static("wrpc:blobstore/blobstore@0.1.0", "get-container-info", name)
    }
    #[instrument(level = "trace", skip_all)]
    fn serve_get_container_info(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            impl Stream<
                Item = anyhow::Result<
                    AcceptedInvocation<
                        <Self as wrpc_transport::Client>::Context,
                        String,
                        <Self as wrpc_transport::Client>::Subject,
                        <<Self as wrpc_transport::Client>::Acceptor as Acceptor>::Transmitter,
                    >,
                >,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "get-container-info")
    }

    #[instrument(level = "trace", skip_all)]
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
            impl Stream<
                Item = anyhow::Result<
                    AcceptedInvocation<
                        Self::Context,
                        (String, Option<u64>, Option<u64>),
                        Self::Subject,
                        <Self::Acceptor as Acceptor>::Transmitter,
                    >,
                >,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "list-container-objects")
    }

    #[instrument(level = "trace", skip_all)]
    fn invoke_copy_object(
        &self,
        src: ObjectId,
        dest: ObjectId,
    ) -> impl Future<Output = anyhow::Result<(Result<(), String>, Self::Transmission)>> + Send {
        self.invoke_static("wrpc:blobstore/blobstore@0.1.0", "copy-object", (src, dest))
    }

    #[instrument(level = "trace", skip_all)]
    fn serve_copy_object(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            impl Stream<
                Item = anyhow::Result<
                    AcceptedInvocation<
                        Self::Context,
                        (ObjectId, ObjectId),
                        Self::Subject,
                        <Self::Acceptor as Acceptor>::Transmitter,
                    >,
                >,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "copy-object")
    }

    #[instrument(level = "trace", skip_all)]
    fn invoke_delete_object(
        &self,
        id: ObjectId,
    ) -> impl Future<Output = anyhow::Result<(Result<(), String>, Self::Transmission)>> + Send {
        self.invoke_static("wrpc:blobstore/blobstore@0.1.0", "delete-object", id)
    }
    #[instrument(level = "trace", skip_all)]
    fn serve_delete_object(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            impl Stream<
                    Item = anyhow::Result<
                        AcceptedInvocation<
                            <Self as wrpc_transport::Client>::Context,
                            ObjectId,
                            <Self as wrpc_transport::Client>::Subject,
                            <<Self as wrpc_transport::Client>::Acceptor as Acceptor>::Transmitter,
                        >,
                    >,
                > + Send,
        >,
    > + Send {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "delete-object")
    }

    #[instrument(level = "trace", skip_all)]
    fn invoke_delete_objects(
        &self,
        container: String,
        objects: Vec<String>,
    ) -> impl Future<Output = anyhow::Result<(Result<(), String>, Self::Transmission)>> + Send {
        self.invoke_static(
            "wrpc:blobstore/blobstore@0.1.0",
            "delete-objects",
            (container, objects),
        )
    }

    #[instrument(level = "trace", skip_all)]
    fn serve_delete_objects(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            impl Stream<
                    Item = anyhow::Result<
                        AcceptedInvocation<
                            <Self as wrpc_transport::Client>::Context,
                            (String, Vec<String>),
                            <Self as wrpc_transport::Client>::Subject,
                            <<Self as wrpc_transport::Client>::Acceptor as Acceptor>::Transmitter,
                        >,
                    >,
                > + Send,
        >,
    > + Send {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "delete-objects")
    }

    #[instrument(level = "trace", skip_all)]
    fn invoke_get_container_data(
        &self,
        id: ObjectId,
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
            impl Stream<
                    Item = anyhow::Result<
                        AcceptedInvocation<
                            <Self as wrpc_transport::Client>::Context,
                            (ObjectId, u64, u64),
                            <Self as wrpc_transport::Client>::Subject,
                            <<Self as wrpc_transport::Client>::Acceptor as Acceptor>::Transmitter,
                        >,
                    >,
                > + Send,
        >,
    > + Send {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "get-container-data")
    }

    #[instrument(level = "trace", skip_all)]
    fn invoke_get_object_info(
        &self,
        id: ObjectId,
    ) -> impl Future<Output = anyhow::Result<(Result<ObjectMetadata, String>, Self::Transmission)>> + Send
    {
        self.invoke_static("wrpc:blobstore/blobstore@0.1.0", "get-object-info", id)
    }

    #[instrument(level = "trace", skip_all)]
    fn serve_get_object_info(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            impl Stream<
                    Item = anyhow::Result<
                        AcceptedInvocation<
                            <Self as wrpc_transport::Client>::Context,
                            ObjectId,
                            <Self as wrpc_transport::Client>::Subject,
                            <<Self as wrpc_transport::Client>::Acceptor as Acceptor>::Transmitter,
                        >,
                    >,
                > + Send,
        >,
    > + Send {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "get-object-info")
    }

    #[instrument(level = "trace", skip_all)]
    fn invoke_has_object(
        &self,
        id: ObjectId,
    ) -> impl Future<Output = anyhow::Result<(Result<bool, String>, Self::Transmission)>> + Send
    {
        self.invoke_static("wrpc:blobstore/blobstore@0.1.0", "has-object", id)
    }

    #[instrument(level = "trace", skip_all)]
    fn serve_has_object(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            impl Stream<
                    Item = anyhow::Result<
                        AcceptedInvocation<
                            <Self as wrpc_transport::Client>::Context,
                            ObjectId,
                            <Self as wrpc_transport::Client>::Subject,
                            <<Self as wrpc_transport::Client>::Acceptor as Acceptor>::Transmitter,
                        >,
                    >,
                > + Send,
        >,
    > + Send {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "has-object")
    }

    #[instrument(level = "trace", skip_all)]
    fn invoke_move_object(
        &self,
        src: ObjectId,
        dest: ObjectId,
    ) -> impl Future<Output = anyhow::Result<(Result<(), String>, Self::Transmission)>> + Send {
        self.invoke_static("wrpc:blobstore/blobstore@0.1.0", "move-object", (src, dest))
    }

    #[instrument(level = "trace", skip_all)]
    fn serve_move_object(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            impl Stream<
                    Item = anyhow::Result<
                        AcceptedInvocation<
                            <Self as wrpc_transport::Client>::Context,
                            (ObjectId, ObjectId),
                            <Self as wrpc_transport::Client>::Subject,
                            <<Self as wrpc_transport::Client>::Acceptor as Acceptor>::Transmitter,
                        >,
                    >,
                > + Send,
        >,
    > + Send {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "move-object")
    }

    #[instrument(level = "trace", skip_all)]
    fn invoke_write_container_data(
        &self,
        id: ObjectId,
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
            impl Stream<
                    Item = anyhow::Result<
                        AcceptedInvocation<
                            <Self as wrpc_transport::Client>::Context,
                            (ObjectId, IncomingInputStream),
                            <Self as wrpc_transport::Client>::Subject,
                            <<Self as wrpc_transport::Client>::Acceptor as Acceptor>::Transmitter,
                        >,
                    >,
                > + Send,
        >,
    > + Send {
        self.serve_static("wrpc:blobstore/blobstore@0.1.0", "write-container-data")
    }
}

impl<T: wrpc_transport::Client> Blobstore for T {}
