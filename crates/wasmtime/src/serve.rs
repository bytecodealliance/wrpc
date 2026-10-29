use core::future::poll_fn;
use core::pin::Pin;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context as _, anyhow};
use futures::stream::FuturesUnordered;
use futures::{Stream, StreamExt as _, TryStreamExt as _};
use tokio::select;
use tokio::sync::{mpsc, oneshot};
use tracing::{Instrument as _, Span, debug, instrument};
use wasmtime::component::types;
use wasmtime::component::{Accessor, ComponentExportIndex, Instance, InstancePre, ResourceType};

use crate::{WrpcView, call, rpc_func_name};

type Task<T> = Box<
    dyn for<'a> FnOnce(&'a Accessor<T>) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> + Send,
>;

/// A [`wasmtime::Store`] shared by concurrent tasks, which are all driven by a single long-running
/// [`wasmtime::Store::run_concurrent`] event loop.
pub struct SharedStore<T: 'static> {
    tx: mpsc::Sender<Task<T>>,
}

impl<T> Clone for SharedStore<T> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
        }
    }
}

impl<T: Send + 'static> SharedStore<T> {
    /// Moves `store` into an event loop and returns a handle to it along with the event loop
    /// future.
    ///
    /// The event loop future must be polled for the tasks to make progress, including the
    /// background work of the guest. It completes once all handles are dropped, all
    /// tasks are done and the guest has no tasks left, or with an error once the store traps,
    /// after which all pending and subsequent [`Self::run`] calls fail.
    pub fn new(
        mut store: wasmtime::Store<T>,
    ) -> (
        Self,
        impl Future<Output = anyhow::Result<()>> + Send + 'static,
    ) {
        let (tx, mut rx) = mpsc::channel::<Task<T>>(1);
        let event_loop = async move {
            store
                .run_concurrent(async |store| {
                    let mut tasks = FuturesUnordered::default();
                    loop {
                        select! {
                            Some(task) = rx.recv() => tasks.push(task(store)),
                            Some(()) = tasks.next() => {}
                            else => break,
                        }
                    }
                    poll_fn(|cx| store.poll_no_interesting_tasks(cx)).await;
                })
                .await
                .map_err(anyhow::Error::from)
                .context("failed to run concurrent event loop")
        };
        (Self { tx }, event_loop)
    }

    /// Runs `f` on the event loop and returns its result.
    ///
    /// Dropping the returned future cancels `f`.
    pub async fn run<R>(
        &self,
        f: impl for<'a> FnOnce(&'a Accessor<T>) -> Pin<Box<dyn Future<Output = R> + Send + 'a>>
        + Send
        + 'static,
    ) -> anyhow::Result<R>
    where
        R: Send + 'static,
    {
        let (mut tx, rx) = oneshot::channel();
        let span = Span::current();
        let task: Task<T> = Box::new(move |store| {
            Box::pin(
                async move {
                    select! {
                        res = f(store) => {
                            _ = tx.send(res);
                        }
                        () = tx.closed() => {}
                    }
                }
                .instrument(span),
            )
        });
        self.tx
            .send(task)
            .await
            .map_err(|_| anyhow!("event loop stopped"))?;
        rx.await.context("event loop stopped")
    }
}

pub trait ServeExt: wrpc_transport::Serve {
    /// Serve [`types::ComponentFunc`] from an [`InstancePre`] instantiating it on each call.
    /// This serving method does not support guest-exported resources.
    #[instrument(level = "trace", skip(self, store, instance_pre, host_resources))]
    fn serve_function<T>(
        &self,
        store: impl Fn() -> wasmtime::Store<T> + Send + 'static,
        instance_pre: InstancePre<T>,
        host_resources: impl Into<
            Arc<HashMap<Box<str>, HashMap<Box<str>, (ResourceType, ResourceType)>>>,
        >,
        ty: types::ComponentFunc,
        instance_name: &str,
        name: &str,
    ) -> impl Future<
        Output = anyhow::Result<
            impl Stream<
                Item = anyhow::Result<(
                    Self::Context,
                    Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'static>>,
                )>,
            > + Send
            + 'static,
        >,
    > + Send
    where
        T: WrpcView + 'static,
    {
        let span = Span::current();
        let host_resources = host_resources.into();
        async move {
            debug!(instance = instance_name, name, "serving function export");
            let component_ty = instance_pre.component();
            let idx = if instance_name.is_empty() {
                None
            } else {
                let idx = component_ty
                    .get_export_index(None, instance_name)
                    .with_context(|| format!("export `{instance_name}` not found"))?;
                Some(idx)
            };
            let idx = component_ty
                .get_export_index(idx.as_ref(), name)
                .with_context(|| format!("export `{name}` not found"))?;

            let params_ty: Arc<[_]> = ty.params().map(|(_, ty)| ty).collect();
            #[cfg(feature = "wasmtime-wasi")]
            let io_streams: Arc<[ResourceType]> = crate::paths::wasi_io_stream_resources(
                component_ty.engine(),
                &component_ty.component_type(),
            )
            .into();
            #[cfg(not(feature = "wasmtime-wasi"))]
            let io_streams: Arc<[ResourceType]> = Arc::default();
            let paths = crate::paths::params_async_paths(params_ty.iter(), &io_streams);
            let invocations = self
                .serve(instance_name, rpc_func_name(name), paths)
                .await?;
            let name = Arc::<str>::from(name);
            let results_ty: Arc<[_]> = ty.results().collect();
            let host_resources = Arc::clone(&host_resources);
            Ok(invocations.map_ok(move |(cx, tx, rx)| {
                let instance_pre = instance_pre.clone();
                let name = Arc::clone(&name);
                let params_ty = Arc::clone(&params_ty);
                let results_ty = Arc::clone(&results_ty);
                let host_resources = Arc::clone(&host_resources);
                let io_streams = Arc::clone(&io_streams);

                let mut store = store();
                (
                    cx,
                    Box::pin(
                        async move {
                            let instance = instance_pre
                                .instantiate_async(&mut store)
                                .await
                                .map_err(anyhow::Error::from)
                                .context("failed to instantiate component")?;
                            let func = instance
                                .get_func(&mut store, idx)
                                .with_context(|| format!("function export `{name}` not found"))?;
                            store
                                .run_concurrent(async |store| {
                                    let res = call(
                                        store,
                                        rx,
                                        tx,
                                        &[],
                                        &host_resources,
                                        &io_streams,
                                        params_ty.iter(),
                                        &results_ty,
                                        func,
                                    )
                                    .await;
                                    if res.is_ok() {
                                        poll_fn(|cx| store.poll_no_interesting_tasks(cx)).await;
                                    }
                                    res
                                })
                                .await
                                .map_err(anyhow::Error::from)
                                .context("failed to run concurrent event loop")??;
                            Ok(())
                        }
                        .instrument(span.clone()),
                    ) as Pin<Box<dyn Future<Output = _> + Send + 'static>>,
                )
            }))
        }
    }

    /// Like [`Self::serve_function`], but with a shared `store` instance.
    /// This is required to allow for serving functions, which operate on guest-exported resources.
    /// Invocations run concurrently on the event loop of the `store`.
    #[instrument(
        level = "trace",
        skip(self, store, instance, guest_resources, host_resources)
    )]
    #[allow(clippy::too_many_arguments)]
    fn serve_function_shared<T>(
        &self,
        store: SharedStore<T>,
        instance: Instance,
        idx: ComponentExportIndex,
        guest_resources: impl Into<Arc<[ResourceType]>>,
        host_resources: impl Into<
            Arc<HashMap<Box<str>, HashMap<Box<str>, (ResourceType, ResourceType)>>>,
        >,
        io_stream_resources: impl Into<Arc<[ResourceType]>>,
        ty: types::ComponentFunc,
        instance_name: &str,
        name: &str,
    ) -> impl Future<
        Output = anyhow::Result<
            impl Stream<
                Item = anyhow::Result<(
                    Self::Context,
                    Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'static>>,
                )>,
            > + Send
            + 'static,
        >,
    > + Send
    where
        T: WrpcView + 'static,
    {
        let span = Span::current();
        let guest_resources = guest_resources.into();
        let host_resources = host_resources.into();
        let io_stream_resources = io_stream_resources.into();
        async move {
            debug!(instance = instance_name, name, "serving function export");
            let params_ty: Arc<[_]> = ty.params().map(|(_, ty)| ty).collect();
            let paths = crate::paths::params_async_paths(params_ty.iter(), &io_stream_resources);
            let invocations = self
                .serve(instance_name, rpc_func_name(name), paths)
                .await?;
            let name = Arc::<str>::from(name);
            let results_ty: Arc<[_]> = ty.results().collect();
            let guest_resources = Arc::clone(&guest_resources);
            let host_resources = Arc::clone(&host_resources);
            Ok(invocations.map_ok(move |(cx, tx, rx)| {
                let name = Arc::clone(&name);
                let params_ty = Arc::clone(&params_ty);
                let results_ty = Arc::clone(&results_ty);
                let guest_resources = Arc::clone(&guest_resources);
                let host_resources = Arc::clone(&host_resources);
                let io_stream_resources = Arc::clone(&io_stream_resources);
                let store = store.clone();
                (
                    cx,
                    Box::pin(
                        async move {
                            store
                                .run(move |store| {
                                    Box::pin(async move {
                                        let func = store
                                            .with(|store| instance.get_func(store, idx))
                                            .with_context(|| {
                                                format!("function export `{name}` not found")
                                            })?;
                                        call(
                                            store,
                                            rx,
                                            tx,
                                            &guest_resources,
                                            &host_resources,
                                            &io_stream_resources,
                                            params_ty.iter(),
                                            &results_ty,
                                            func,
                                        )
                                        .await?;
                                        anyhow::Ok(())
                                    })
                                })
                                .await??;
                            Ok(())
                        }
                        .instrument(span.clone()),
                    ) as Pin<Box<dyn Future<Output = _> + Send + 'static>>,
                )
            }))
        }
    }
}

impl<T: wrpc_transport::Serve> ServeExt for T {}
