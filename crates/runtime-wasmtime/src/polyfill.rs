use core::iter::zip;
use core::pin::pin;

use std::sync::Arc;

use anyhow::{bail, Context as _};
use bytes::BytesMut;
use futures::future::try_join_all;
use tokio::io::AsyncWriteExt as _;
use tokio::time::Instant;
use tokio::try_join;
use tokio_util::codec::Encoder;
use tracing::{debug, instrument, trace, warn, Instrument as _, Span};
use wasmtime::component::types;
use wasmtime::component::{LinkerInstance, ResourceType};
use wasmtime::{AsContextMut, Engine};
use wasmtime_wasi::WasiView;
use wrpc_transport::{Index as _, Invoke, InvokeExt as _};

use crate::{read_value, rpc_func_name, RemoteResource, ValEncoder, WrpcView};

/// Polyfill [`types::ComponentItem`] in a [`LinkerInstance`] using [`wrpc_transport::Invoke`]
#[instrument(level = "trace", skip_all)]
pub fn link_item<V>(
    engine: &Engine,
    linker: &mut LinkerInstance<V>,
    resources: impl Into<Arc<[ResourceType]>>,
    ty: types::ComponentItem,
    instance: impl Into<Arc<str>>,
    name: impl Into<Arc<str>>,
) -> wasmtime::Result<()>
where
    V: WasiView + WrpcView,
{
    let instance = instance.into();
    let resources = resources.into();
    match ty {
        types::ComponentItem::ComponentFunc(ty) => {
            let name = name.into();
            debug!(?instance, ?name, "linking function");
            link_function(linker, Arc::clone(&resources), ty, instance, name)?;
        }
        types::ComponentItem::CoreFunc(_) => {
            bail!("polyfilling core functions not supported yet")
        }
        types::ComponentItem::Module(_) => bail!("polyfilling modules not supported yet"),
        types::ComponentItem::Component(ty) => {
            for (name, ty) in ty.imports(engine) {
                debug!(?instance, name, "linking component item");
                link_item(engine, linker, Arc::clone(&resources), ty, "", name)?;
            }
        }
        types::ComponentItem::ComponentInstance(ty) => {
            let name = name.into();
            let mut linker = linker
                .instance(&name)
                .with_context(|| format!("failed to instantiate `{name}` in the linker"))?;
            debug!(?instance, ?name, "linking instance");
            link_instance(engine, &mut linker, resources, ty, name)?;
        }
        types::ComponentItem::Type(_) => {}
        types::ComponentItem::Resource(_) => {
            let name = name.into();
            debug!(?instance, ?name, "linking resource");
            linker.resource(&name, ResourceType::host::<RemoteResource>(), |_, _| Ok(()))?;
        }
    }
    Ok(())
}

/// Polyfill [`types::ComponentInstance`] in a [`LinkerInstance`] using [`wrpc_transport::Invoke`]
#[instrument(level = "trace", skip_all)]
pub fn link_instance<V>(
    engine: &Engine,
    linker: &mut LinkerInstance<V>,
    resources: impl Into<Arc<[ResourceType]>>,
    ty: types::ComponentInstance,
    name: impl Into<Arc<str>>,
) -> wasmtime::Result<()>
where
    V: WrpcView + WasiView,
{
    let instance = name.into();
    let resources = resources.into();
    for (name, ty) in ty.exports(engine) {
        debug!(name, "linking instance item");
        link_item(
            engine,
            linker,
            Arc::clone(&resources),
            ty,
            Arc::clone(&instance),
            name,
        )?;
    }
    Ok(())
}

/// Polyfill [`types::ComponentFunc`] in a [`LinkerInstance`] using [`wrpc_transport::Invoke`]
#[instrument(level = "trace", skip_all)]
pub fn link_function<V>(
    linker: &mut LinkerInstance<V>,
    resources: impl Into<Arc<[ResourceType]>>,
    ty: types::ComponentFunc,
    instance: impl Into<Arc<str>>,
    name: impl Into<Arc<str>>,
) -> wasmtime::Result<()>
where
    V: WrpcView + WasiView,
{
    let span = Span::current();
    let instance = instance.into();
    let name = name.into();
    let resources = resources.into();
    linker.func_new_async(&Arc::clone(&name), move |mut store, params, results| {
        let cx = store.data().context();
        let ty = ty.clone();
        let instance = Arc::clone(&instance);
        let name = Arc::clone(&name);
        let resources = Arc::clone(&resources);
        Box::new(
            async move {
                let mut buf = BytesMut::default();
                let mut deferred = vec![];
                for (v, (_, ref ty)) in zip(params, ty.params()) {
                    let mut enc = ValEncoder::new(store.as_context_mut(), ty, &resources);
                    enc.encode(v, &mut buf)
                        .context("failed to encode parameter")?;
                    deferred.push(enc.deferred);
                }
                let clt = store.data().client();
                let timeout = store.data().timeout();
                let buf = buf.freeze();
                // TODO: set paths
                let paths = &[[]; 0];
                let rpc_name = rpc_func_name(&name);
                let start = Instant::now();
                let (outgoing, incoming) = if let Some(timeout) = timeout {
                    clt.timeout(timeout)
                        .invoke(cx, &instance, rpc_name, buf, paths)
                        .await
                } else {
                    clt.invoke(cx, &instance, rpc_name, buf, paths).await
                }
                .with_context(|| {
                    format!("failed to invoke `{instance}.{name}` polyfill via wRPC")
                })?;
                let tx = async {
                    try_join_all(
                        zip(0.., deferred)
                            .filter_map(|(i, f)| f.map(|f| (outgoing.index(&[i]), f)))
                            .map(|(w, f)| async move {
                                let w = w?;
                                f(w).await
                            }),
                    )
                    .await
                    .context("failed to write asynchronous parameters")?;
                    let mut outgoing = pin!(outgoing);
                    outgoing
                        .flush()
                        .await
                        .context("failed to flush outgoing stream")?;
                    if let Err(err) = outgoing.shutdown().await {
                        trace!(?err, "failed to shutdown outgoing stream");
                    }
                    anyhow::Ok(())
                };
                let rx = async {
                    let mut incoming = pin!(incoming);
                    for (i, (v, ref ty)) in zip(results, ty.results()).enumerate() {
                        read_value(&mut store, &mut incoming, &resources, v, ty, &[i])
                            .await
                            .with_context(|| format!("failed to decode return value {i}"))?;
                    }
                    Ok(())
                };
                if let Some(timeout) = timeout {
                    let timeout =
                        timeout.saturating_sub(Instant::now().saturating_duration_since(start));
                    try_join!(
                        async {
                            tokio::time::timeout(timeout, tx)
                                .await
                                .context("data transmission timed out")?
                        },
                        async {
                            tokio::time::timeout(timeout, rx)
                                .await
                                .context("data receipt timed out")?
                        },
                    )?;
                } else {
                    try_join!(tx, rx)?;
                }
                Ok(())
            }
            .instrument(span.clone()),
        )
    })
}
