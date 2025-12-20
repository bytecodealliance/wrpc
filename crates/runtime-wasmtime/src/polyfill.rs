use core::iter::zip;
use core::pin::pin;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, ensure, Context as _};
use bytes::BytesMut;
use futures::future::try_join_all;
use tokio::io::AsyncWriteExt as _;
use tokio::time::Instant;
use tokio::try_join;
use tokio_util::codec::Encoder;
use tracing::{debug, instrument, trace, warn, Instrument as _, Span};
use wasmtime::component::{types, LinkerInstance, ResourceType, Type, Val};
use wasmtime::{AsContextMut, Engine, StoreContextMut};
use wrpc_transport::{Index as _, Invoke, InvokeExt as _};

use crate::rpc::Error;
use crate::{read_value, rpc_func_name, rpc_result_type, ValEncoder, WrpcView, WrpcViewExt as _};

/// Polyfill [`types::ComponentItem`] in a [`LinkerInstance`] using [`wrpc_transport::Invoke`]
#[instrument(level = "trace", skip_all)]
pub fn link_item<V>(
    engine: &Engine,
    linker: &mut LinkerInstance<V>,
    guest_resources: impl Into<Arc<[ResourceType]>>,
    host_resources: impl Into<Arc<HashMap<Box<str>, HashMap<Box<str>, (ResourceType, ResourceType)>>>>,
    ty: types::ComponentItem,
    instance: impl Into<Arc<str>>,
    name: impl Into<Arc<str>>,
) -> wasmtime::Result<()>
where
    V: WrpcView,
{
    let instance = instance.into();
    let guest_resources = guest_resources.into();
    let host_resources = host_resources.into();
    match ty {
        types::ComponentItem::ComponentFunc(ty) => {
            let name = name.into();
            debug!(?instance, ?name, "linking function");
            link_function(
                linker,
                Arc::clone(&guest_resources),
                Arc::clone(&host_resources),
                ty,
                instance,
                name,
            )?;
        }
        types::ComponentItem::CoreFunc(_) => {
            bail!("polyfilling core functions not supported yet")
        }
        types::ComponentItem::Module(_) => bail!("polyfilling modules not supported yet"),
        types::ComponentItem::Component(ty) => {
            for (name, ty) in ty.imports(engine) {
                debug!(?instance, name, "linking component item");
                link_item(
                    engine,
                    linker,
                    Arc::clone(&guest_resources),
                    Arc::clone(&host_resources),
                    ty,
                    "",
                    name,
                )?;
            }
        }
        types::ComponentItem::ComponentInstance(ty) => {
            let name = name.into();
            let mut linker = linker
                .instance(&name)
                .with_context(|| format!("failed to instantiate `{name}` in the linker"))?;
            debug!(?instance, ?name, "linking instance");
            link_instance(
                engine,
                &mut linker,
                guest_resources,
                host_resources,
                ty,
                name,
            )?;
        }
        types::ComponentItem::Type(_) => {}
        types::ComponentItem::Resource(ty) => {
            let name = name.into();
            let Some((guest_ty, host_ty)) = host_resources
                .get(&*instance)
                .and_then(|instance| instance.get(&*name))
            else {
                bail!("resource type for {instance}/{name} not defined");
            };
            ensure!(ty == *guest_ty, "{instance}/{name} resource type mismatch");

            debug!(?instance, ?name, "linking resource");
            linker.resource(&name, *host_ty, |_, _| Ok(()))?;
        }
    }
    Ok(())
}

/// Polyfill [`types::ComponentInstance`] in a [`LinkerInstance`] using [`wrpc_transport::Invoke`]
#[instrument(level = "trace", skip_all)]
pub fn link_instance<V>(
    engine: &Engine,
    linker: &mut LinkerInstance<V>,
    guest_resources: impl Into<Arc<[ResourceType]>>,
    host_resources: impl Into<Arc<HashMap<Box<str>, HashMap<Box<str>, (ResourceType, ResourceType)>>>>,
    ty: types::ComponentInstance,
    name: impl Into<Arc<str>>,
) -> wasmtime::Result<()>
where
    V: WrpcView,
{
    let instance = name.into();
    let guest_resources = guest_resources.into();
    let host_resources = host_resources.into();
    for (name, ty) in ty.exports(engine) {
        debug!(name, "linking instance item");
        link_item(
            engine,
            linker,
            Arc::clone(&guest_resources),
            Arc::clone(&host_resources),
            ty,
            Arc::clone(&instance),
            name,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn invoke<T: WrpcView>(
    mut store: &mut StoreContextMut<'_, T>,
    params: &[Val],
    results: &mut [Val],
    guest_resources: Arc<[ResourceType]>,
    params_ty: impl IntoIterator<Item = (&str, Type)>,
    results_ty: impl IntoIterator<Item = Type>,
    instance: Arc<str>,
    name: Arc<str>,
) -> wasmtime::Result<anyhow::Result<()>> {
    let mut buf = BytesMut::default();
    let mut deferred = vec![];
    for (v, (name, ref ty)) in zip(params, params_ty) {
        let mut enc = ValEncoder::new(store.as_context_mut(), ty, &guest_resources);
        enc.encode(v, &mut buf)
            .with_context(|| format!("failed to encode parameter `{name}`"))?;
        deferred.push(enc.deferred);
    }
    let view = store.data_mut().wrpc();
    let clt = view.ctx.client();
    let cx = view.ctx.context();
    let timeout = view.ctx.timeout();
    let buf = buf.freeze();
    // TODO: set paths
    let paths = &[[]; 0];
    let rpc_name = rpc_func_name(&name);
    let start = Instant::now();
    let invocation = if let Some(timeout) = timeout {
        clt.timeout(timeout)
            .invoke(cx, &instance, rpc_name, buf, paths)
            .await
    } else {
        clt.invoke(cx, &instance, rpc_name, buf, paths).await
    }
    .with_context(|| format!("failed to invoke `{instance}.{name}` polyfill via wRPC"));
    let (outgoing, incoming) = match invocation {
        Ok((outgoing, incoming)) => (outgoing, incoming),
        Err(err) => return Ok(Err(err)),
    };
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
        for (i, (v, ref ty)) in zip(results, results_ty).enumerate() {
            read_value(&mut store, &mut incoming, &guest_resources, v, ty, &[i])
                .await
                .with_context(|| format!("failed to decode return value {i}"))?;
        }
        Ok(())
    };
    let res = if let Some(timeout) = timeout {
        let timeout = timeout.saturating_sub(Instant::now().saturating_duration_since(start));
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
        )
    } else {
        try_join!(tx, rx)
    };
    match res {
        Ok(((), ())) => Ok(Ok(())),
        Err(err) => Ok(Err(err)),
    }
}

/// Polyfill [`types::ComponentFunc`] in a [`LinkerInstance`] using [`wrpc_transport::Invoke`]
#[instrument(level = "trace", skip_all)]
pub fn link_function<V>(
    linker: &mut LinkerInstance<V>,
    guest_resources: impl Into<Arc<[ResourceType]>>,
    host_resources: impl Into<Arc<HashMap<Box<str>, HashMap<Box<str>, (ResourceType, ResourceType)>>>>,
    ty: types::ComponentFunc,
    instance: impl Into<Arc<str>>,
    name: impl Into<Arc<str>>,
) -> wasmtime::Result<()>
where
    V: WrpcView,
{
    let span = Span::current();
    let instance = instance.into();
    let name = name.into();
    let guest_resources = guest_resources.into();
    let host_resources = host_resources.into();
    match rpc_result_type(&host_resources, ty.results()) {
        None => linker.func_new_async(&Arc::clone(&name), move |mut store, ty, params, results| {
            let instance = Arc::clone(&instance);
            let name = Arc::clone(&name);
            let resources = Arc::clone(&guest_resources);
            Box::new(
                async move {
                    match invoke(
                        &mut store,
                        params,
                        results,
                        resources,
                        ty.params(),
                        ty.results(),
                        instance,
                        name,
                    )
                    .await
                    {
                        Ok(Ok(())) => Ok(()),
                        Ok(Err(err)) => Err(err),
                        Err(err) => Err(err),
                    }
                }
                .instrument(span.clone()),
            )
        }),
        // `result<_, rpc-eror>`
        Some(None) => {
            linker.func_new_async(&Arc::clone(&name), move |mut store, ty, params, results| {
                let instance = Arc::clone(&instance);
                let name = Arc::clone(&name);
                let resources = Arc::clone(&guest_resources);
                Box::new(
                    async move {
                        let [result] = results else {
                            bail!("result type mismatch");
                        };
                        match invoke(
                            &mut store,
                            params,
                            &mut [],
                            resources,
                            ty.params(),
                            None,
                            instance,
                            name,
                        )
                        .await?
                        {
                            Ok(()) => {
                                *result = Val::Result(Ok(None));
                            }
                            Err(err) => {
                                let err = store.data_mut().push_error(Error::Invoke(err))?;
                                let err = err
                                    .try_into_resource_any(&mut store)
                                    .context("failed to lower error resource")?;
                                *result = Val::Result(Err(Some(Box::new(Val::Resource(err)))));
                            }
                        }
                        Ok(())
                    }
                    .instrument(span.clone()),
                )
            })
        }
        // `result<T, rpc-eror>`
        Some(Some(result_ty)) => {
            linker.func_new_async(&Arc::clone(&name), move |mut store, ty, params, results| {
                let instance = Arc::clone(&instance);
                let name = Arc::clone(&name);
                let resources = Arc::clone(&guest_resources);
                let result_ty = result_ty.clone();
                Box::new(
                    async move {
                        let [result] = results else {
                            bail!("result type mismatch");
                        };
                        let mut ok = [Val::Bool(false); 1];
                        match invoke(
                            &mut store,
                            params,
                            ok.as_mut_slice(),
                            resources,
                            ty.params(),
                            [result_ty],
                            instance,
                            name,
                        )
                        .await?
                        {
                            Ok(()) => {
                                let [ok] = ok;
                                *result = Val::Result(Ok(Some(Box::new(ok))));
                            }
                            Err(err) => {
                                let err = store.data_mut().push_error(Error::Invoke(err))?;
                                let err = err
                                    .try_into_resource_any(&mut store)
                                    .context("failed to lower error resource")?;
                                *result = Val::Result(Err(Some(Box::new(Val::Resource(err)))));
                            }
                        }
                        Ok(())
                    }
                    .instrument(span.clone()),
                )
            })
        }
    }
}
