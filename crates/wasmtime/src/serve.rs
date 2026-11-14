use core::future::poll_fn;
use core::mem;
use core::pin::Pin;
use core::time::Duration;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context as _, anyhow};
use futures::stream::FuturesUnordered;
use futures::{StreamExt as _, TryStreamExt as _};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot};
use tokio::{select, time};
use tokio_util::codec::{Decoder as _, FramedRead};
use tracing::{Instrument as _, Span, debug, warn};
use wasmtime::component::{
    Accessor, Component, ComponentExportIndex, Instance, InstancePre, Type, types,
};
use wrpc_transport::{FrameDecoder, Header, HeaderDecoder, Server};

use crate::{ResourceTypes, TupleDecoder, TupleEncoder, WrpcView, call};

// this returns the RPC name for a wasmtime function name.
// Unfortunately, the [`types::ComponentFunc`] does not include the kind information and we want to
// avoid (re-)parsing the WIT here.
fn rpc_func_name(name: &str) -> &str {
    if let Some(name) = name.strip_prefix("[constructor]") {
        name
    } else if let Some(name) = name.strip_prefix("[static]") {
        name
    } else if let Some(name) = name.strip_prefix("[method]") {
        name
    } else {
        name
    }
}

/// A [`wasmtime::Store`] shared by concurrent tasks, which are all driven by a single long-running
/// [`wasmtime::Store::run_concurrent`] event loop.
pub struct SharedStore<T: 'static> {
    tx: mpsc::Sender<
        Box<
            dyn for<'a> FnOnce(&'a Accessor<T>) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>
                + Send,
        >,
    >,
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
        let (tx, mut rx) = mpsc::channel::<
            Box<
                dyn for<'a> FnOnce(&'a Accessor<T>) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>
                    + Send,
            >,
        >(1);
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
        let task: Box<
            dyn for<'a> FnOnce(&'a Accessor<T>) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>
                + Send,
        > = Box::new(move |store| {
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
}

fn exports(
    component: &Component,
) -> anyhow::Result<HashMap<Box<str>, HashMap<Box<str>, Arc<Export>>>> {
    let engine = component.engine();
    let mut exports = HashMap::<Box<str>, HashMap<Box<str>, Arc<Export>>>::default();
    let mut insert = |instance: &str,
                      instance_idx: Option<&ComponentExportIndex>,
                      name: &str,
                      ty: types::ComponentFunc|
     -> anyhow::Result<()> {
        let idx = component
            .get_export_index(instance_idx, name)
            .with_context(|| format!("export `{name}` not found"))?;
        let params: Box<[_]> = ty.params().map(|(.., ty)| ty).collect();
        debug!(instance, name, "serving function export");
        exports.entry(instance.into()).or_default().insert(
            rpc_func_name(name).into(),
            Arc::new(Export {
                idx,
                params,
                results: ty.results().collect(),
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

fn lookup(
    exports: &HashMap<Box<str>, HashMap<Box<str>, Arc<Export>>>,
    instance: &str,
    func: &str,
) -> anyhow::Result<Arc<Export>> {
    exports
        .get(instance)
        .and_then(|funcs| funcs.get(func))
        .map(Arc::clone)
        .with_context(|| format!("`{instance}#{func}` is not exported"))
}

/// Serves the invocations sent over the byte streams `tx` and `rx` one after another using `f`
async fn serve<I, O, Fut>(
    exports: &HashMap<Box<str>, HashMap<Box<str>, Arc<Export>>>,
    mut tx: O,
    rx: I,
    timeout: Option<Duration>,
    mut f: impl FnMut(Arc<Export>, O, FramedRead<I, FrameDecoder>) -> Fut,
) -> anyhow::Result<()>
where
    I: AsyncRead + Unpin,
    Fut: Future<Output = anyhow::Result<(O, FramedRead<I, FrameDecoder>)>>,
{
    let mut rx = FramedRead::new(rx, HeaderDecoder::default());
    loop {
        let header = match timeout {
            Some(timeout) => time::timeout(timeout, rx.try_next()).await,
            None => Ok(rx.try_next().await),
        };
        let Ok(header) = header else {
            let mut buf = mem::take(rx.read_buffer_mut());
            rx.decoder_mut()
                .decode_eof(&mut buf)
                .context("timed out reading invocation header")?;
            return Ok(());
        };
        let Some(Header { instance, name }) = header.context("failed to read invocation header")?
        else {
            return Ok(());
        };
        let export = lookup(exports, &instance, &name)?;
        let frames;
        (tx, frames) = f(export, tx, rx.map_decoder(|_| FrameDecoder::default())).await?;
        rx = frames.map_decoder(|_| HeaderDecoder::default());
    }
}

/// Function exports of a component served from an [`InstancePre`], which is instantiated in a
/// new store on each invocation. Guest-exported resources are not supported.
pub struct InstancePreExports<T: 'static, F> {
    new_store: F,
    pre: InstancePre<T>,
    exports: HashMap<Box<str>, HashMap<Box<str>, Arc<Export>>>,
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
            io_streams: crate::wasi_io_stream_resources(
                component.engine(),
                &component.component_type(),
            )
            .into(),
            ..ResourceTypes::default()
        };
        let exports = exports(component)?;
        Ok(Self {
            new_store,
            pre,
            exports,
            resources,
        })
    }

    /// Serves the invocations sent over the byte streams `tx` and `rx` of a connection one after
    /// another, until the client ends transmit, an invocation fails or the connection stays idle
    /// for longer than `timeout`
    ///
    /// # Errors
    ///
    /// Returns an error if serving an invocation fails or `timeout` elapses within a header
    pub async fn serve<I, O>(&self, tx: O, rx: I, timeout: Option<Duration>) -> anyhow::Result<()>
    where
        I: AsyncRead + Unpin,
        O: AsyncWrite + Unpin,
    {
        serve(
            &self.exports,
            tx,
            rx,
            timeout,
            |export, mut tx, mut frames| async move {
                let mut store = (self.new_store)();
                let instance = self
                    .pre
                    .instantiate_async(&mut store)
                    .await
                    .map_err(anyhow::Error::from)
                    .context("failed to instantiate component")?;
                let func = instance
                    .get_func(&mut store, export.idx)
                    .context("function export not found")?;
                store
                    .run_concurrent(async |store| {
                        let srv = Server {
                            enc: TupleEncoder::new(store, &export.results, &self.resources),
                            dec: TupleDecoder::new(store, &export.params, &self.resources),
                        };
                        let res = call(store, srv, &mut tx, &mut frames, func).await;
                        if res.is_ok() {
                            poll_fn(|cx| store.poll_no_interesting_tasks(cx)).await;
                        }
                        res
                    })
                    .await
                    .map_err(anyhow::Error::from)
                    .context("failed to run concurrent event loop")??;
                Ok((tx, frames))
            },
        )
        .await
    }
}

/// Function exports of an [`Instance`] served on a [`SharedStore`], which is required to serve
/// functions operating on guest-exported resources. Invocations run concurrently on the event
/// loop of the store.
pub struct SharedExports<T: 'static> {
    store: SharedStore<T>,
    instance: Instance,
    exports: HashMap<Box<str>, HashMap<Box<str>, Arc<Export>>>,
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
        let exports = exports(component)?;
        Ok(Self {
            store,
            instance,
            exports,
            resources,
        })
    }

    /// Serves the invocations sent over the byte streams `tx` and `rx` of a connection one after
    /// another, until the client ends transmit, an invocation fails or the connection stays idle
    /// for longer than `timeout`
    ///
    /// # Errors
    ///
    /// Returns an error if serving an invocation fails or `timeout` elapses within a header
    pub async fn serve<I, O>(&self, tx: O, rx: I, timeout: Option<Duration>) -> anyhow::Result<()>
    where
        I: AsyncRead + Send + Unpin + 'static,
        O: AsyncWrite + Send + Unpin + 'static,
    {
        serve(
            &self.exports,
            tx,
            rx,
            timeout,
            |export, mut tx, mut frames| async move {
                let instance = self.instance;
                let resources = self.resources.clone();
                self.store
                    .run(move |store| {
                        Box::pin(async move {
                            let func = store
                                .with(|store| instance.get_func(store, export.idx))
                                .context("function export not found")?;
                            let srv = Server {
                                enc: TupleEncoder::new(store, &export.results, &resources),
                                dec: TupleDecoder::new(store, &export.params, &resources),
                            };
                            call(store, srv, &mut tx, &mut frames, func).await?;
                            Ok((tx, frames))
                        })
                    })
                    .await?
            },
        )
        .await
    }
}
