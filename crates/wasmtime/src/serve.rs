use core::future::poll_fn;
use core::pin::{Pin, pin};

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context as _, anyhow};
use bytes::BytesMut;
use futures::StreamExt as _;
use futures::stream::FuturesUnordered;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::select;
use tokio::sync::{mpsc, oneshot};
use tracing::{Instrument as _, Span, debug, warn};
use wasmtime::component::{
    Accessor, Component, ComponentExportIndex, Instance, InstancePre, Type, types,
};
use wrpc_transport::frame::{Incoming, Outgoing};
use wrpc_transport::serve;

use crate::{CallError, ResourceTypes, WrpcView, call, rpc_func_name};

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

struct Export {
    idx: ComponentExportIndex,
    params: Box<[Type]>,
    results: Box<[Type]>,
    paths: Arc<[Box<[Option<usize>]>]>,
}

type Exports = HashMap<Box<str>, HashMap<Box<str>, Arc<Export>>>;

fn exports(component: &Component, resources: &ResourceTypes) -> anyhow::Result<Exports> {
    let engine = component.engine();
    let mut exports = Exports::default();
    let mut insert = |instance: &str,
                      instance_idx: Option<&ComponentExportIndex>,
                      name: &str,
                      ty: types::ComponentFunc|
     -> anyhow::Result<()> {
        let idx = component
            .get_export_index(instance_idx, name)
            .with_context(|| format!("export `{name}` not found"))?;
        let params: Box<[_]> = ty.params().map(|(.., ty)| ty).collect();
        let paths = crate::paths::params_async_paths(params.iter(), resources);
        debug!(instance, name, "serving function export");
        exports.entry(instance.into()).or_default().insert(
            rpc_func_name(name).into(),
            Arc::new(Export {
                idx,
                params,
                results: ty.results().collect(),
                paths,
            }),
        );
        Ok(())
    };
    for (name, types::ComponentExtern { ty, .. }) in component.component_type().exports(engine) {
        match ty {
            types::ComponentItem::ComponentFunc(ty) => insert("", None, name, ty)?,
            types::ComponentItem::ComponentInstance(ty) => {
                let idx = component
                    .get_export_index(None, name)
                    .with_context(|| format!("export `{name}` not found"))?;
                for (func, types::ComponentExtern { ty, .. }) in ty.exports(engine) {
                    match ty {
                        types::ComponentItem::ComponentFunc(ty) => {
                            insert(name, Some(&idx), func, ty)?;
                        }
                        types::ComponentItem::Type(..) | types::ComponentItem::Resource(..) => {}
                        _ => warn!(instance = name, name = func, "export kind not supported"),
                    }
                }
            }
            types::ComponentItem::Type(..) | types::ComponentItem::Resource(..) => {}
            _ => warn!(name, "export kind not supported"),
        }
    }
    Ok(exports)
}

fn lookup<C, I, O>(
    exports: &Exports,
    cx: C,
    instance: &str,
    func: &str,
    tx: O,
    rx: I,
    buf: BytesMut,
) -> Result<(Arc<Export>, Outgoing, Incoming), (C, O, I, BytesMut)>
where
    I: AsyncRead + Send + Unpin + 'static,
    O: AsyncWrite + Send + Unpin + 'static,
{
    let Some(export) = exports.get(instance).and_then(|funcs| funcs.get(func)) else {
        return Err((cx, tx, rx, buf));
    };
    let export = Arc::clone(export);
    let (tx, rx) = serve(tx, rx, buf, export.paths.iter());
    Ok((export, tx, rx))
}

/// Function exports of a component served from an [`InstancePre`], which is instantiated in a
/// new store on each invocation. Guest-exported resources are not supported.
pub struct InstancePreExports<T: 'static, F> {
    new_store: F,
    pre: InstancePre<T>,
    exports: Exports,
    resources: ResourceTypes,
}

impl<T, F> InstancePreExports<T, F>
where
    T: WrpcView + 'static,
    F: Fn() -> wasmtime::Store<T>,
{
    /// Collects the function exports of the component of `pre`, each invocation of which
    /// is served in a store returned by `new_store`.
    ///
    /// # Errors
    ///
    /// Returns an error if an export cannot be looked up
    pub fn new(new_store: F, pre: InstancePre<T>) -> anyhow::Result<Self> {
        let component = pre.component();
        let resources = ResourceTypes {
            #[cfg(feature = "wasmtime-wasi")]
            io_streams: crate::paths::wasi_io_stream_resources(
                component.engine(),
                &component.component_type(),
            )
            .into(),
            ..ResourceTypes::default()
        };
        let exports = exports(component, &resources)?;
        Ok(Self {
            new_store,
            pre,
            exports,
            resources,
        })
    }

    /// Returns a future serving the invocation of function `func` on instance `instance` over
    /// the byte streams `tx` and `rx`, whose header was read by
    /// [`accept`](wrpc_transport::accept()) along with the bytes in `buf`, or the context `cx`,
    /// the byte streams and `buf` back if it does not invoke an exported function
    pub fn handle<C, I, O>(
        &self,
        cx: C,
        instance: &str,
        func: &str,
        tx: O,
        rx: I,
        buf: BytesMut,
    ) -> Result<
        Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'static>>,
        (C, O, I, BytesMut),
    >
    where
        I: AsyncRead + Send + Unpin + 'static,
        O: AsyncWrite + Send + Unpin + 'static,
    {
        let (export, tx, rx) = lookup(&self.exports, cx, instance, func, tx, rx, buf)?;
        let mut store = (self.new_store)();
        let pre = self.pre.clone();
        let resources = self.resources.clone();
        Ok(Box::pin(
            async move {
                let instance = pre
                    .instantiate_async(&mut store)
                    .await
                    .map_err(anyhow::Error::from)
                    .context("failed to instantiate component")?;
                let func = instance
                    .get_func(&mut store, export.idx)
                    .context("function export not found")?;
                let io = store
                    .run_concurrent(async |store| {
                        let io = call(
                            store,
                            rx,
                            tx,
                            &resources,
                            &export.params,
                            &export.results,
                            func,
                        )
                        .await?;
                        let mut io = Box::pin(io);
                        let mut done = pin!(poll_fn(|cx| store.poll_no_interesting_tasks(cx)));
                        select! {
                            res = &mut io => {
                                res?;
                                done.await;
                                Ok::<_, CallError>(None)
                            }
                            () = &mut done => Ok(Some(io)),
                        }
                    })
                    .await
                    .map_err(anyhow::Error::from)
                    .context("failed to run concurrent event loop")??;
                drop(store);
                if let Some(io) = io {
                    io.await?;
                }
                Ok(())
            }
            .in_current_span(),
        ))
    }
}

/// Function exports of an [`Instance`] served on a [`SharedStore`], which is required to serve
/// functions operating on guest-exported resources. Invocations run concurrently on the event
/// loop of the store.
pub struct SharedExports<T: 'static> {
    store: SharedStore<T>,
    instance: Instance,
    exports: Exports,
    resources: ResourceTypes,
}

impl<T> SharedExports<T>
where
    T: WrpcView + 'static,
{
    /// Collects the function exports of `component`, which `instance` is an instance of.
    ///
    /// # Errors
    ///
    /// Returns an error if an export cannot be looked up
    pub fn new(
        store: SharedStore<T>,
        instance: Instance,
        component: &Component,
        resources: ResourceTypes,
    ) -> anyhow::Result<Self> {
        let exports = exports(component, &resources)?;
        Ok(Self {
            store,
            instance,
            exports,
            resources,
        })
    }

    /// Returns a future serving the invocation of function `func` on instance `instance` over
    /// the byte streams `tx` and `rx`, whose header was read by
    /// [`accept`](wrpc_transport::accept()) along with the bytes in `buf`, or the context `cx`,
    /// the byte streams and `buf` back if it does not invoke an exported function
    pub fn handle<C, I, O>(
        &self,
        cx: C,
        instance: &str,
        func: &str,
        tx: O,
        rx: I,
        buf: BytesMut,
    ) -> Result<
        Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'static>>,
        (C, O, I, BytesMut),
    >
    where
        I: AsyncRead + Send + Unpin + 'static,
        O: AsyncWrite + Send + Unpin + 'static,
    {
        let (export, tx, rx) = lookup(&self.exports, cx, instance, func, tx, rx, buf)?;
        let store = self.store.clone();
        let instance = self.instance;
        let resources = self.resources.clone();
        Ok(Box::pin(
            async move {
                let io = store
                    .run(move |store| {
                        Box::pin(async move {
                            let func = store
                                .with(|store| instance.get_func(store, export.idx))
                                .context("function export not found")?;
                            let io = call(
                                store,
                                rx,
                                tx,
                                &resources,
                                &export.params,
                                &export.results,
                                func,
                            )
                            .await?;
                            anyhow::Ok(io)
                        })
                    })
                    .await??;
                drop(store);
                io.await?;
                Ok(())
            }
            .in_current_span(),
        ))
    }
}
