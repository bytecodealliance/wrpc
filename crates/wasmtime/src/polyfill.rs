use core::iter::zip;
use core::pin::pin;

use std::collections::HashMap;
use std::sync::Arc;

use bytes::BytesMut;
use futures::future::try_join_all;
use tokio::io::AsyncWriteExt as _;
use tokio::time::Instant;
use tokio::try_join;
use tokio_util::codec::Encoder;
use tracing::{Instrument as _, Span, debug, instrument, warn};
use wasmtime::component::{LinkerInstance, ResourceType, Type, Val, types};
use wasmtime::error::Context as _;
use wasmtime::{AsContextMut, Engine, bail, ensure};
use wrpc_transport::{Invoke, InvokeExt as _};

use crate::paths::params_async_paths;
use crate::rpc::Error;
use crate::{
    ResourceTypes, ValEncoder, WithStore, WrpcView, WrpcViewExt as _, read_value, rpc_func_name,
    rpc_result_type,
};

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
    V: WrpcView<Invoke: Clone> + 'static,
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
            for (name, types::ComponentExtern { ty, .. }) in ty.imports(engine) {
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
    V: WrpcView<Invoke: Clone> + 'static,
{
    let instance = name.into();
    let guest_resources = guest_resources.into();
    let host_resources = host_resources.into();
    for (name, types::ComponentExtern { ty, .. }) in ty.exports(engine) {
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
async fn invoke(
    store: &mut impl WithStore<Data: WrpcView<Invoke: Clone>>,
    params: &[Val],
    results: &mut [Val],
    resources: &ResourceTypes,
    params_ty: impl IntoIterator<Item = (&str, Type)>,
    results_ty: impl IntoIterator<Item = Type>,
    instance: Arc<str>,
    name: Arc<str>,
) -> wasmtime::Result<anyhow::Result<()>> {
    let (buf, deferred, clt, cx, timeout) = store.with_store(|mut store| {
        let mut buf = BytesMut::default();
        let mut deferred = vec![];
        for (v, (name, ref ty)) in zip(params, params_ty) {
            let mut enc = ValEncoder::new(store.as_context_mut(), ty, resources);
            enc.encode(v, &mut buf)
                .with_context(|| format!("failed to encode parameter `{name}`"))?;
            deferred.push(enc.deferred);
        }
        let view = store.data_mut().wrpc();
        wasmtime::error::Ok((
            buf.freeze(),
            deferred,
            view.ctx.client().clone(),
            view.ctx.context(),
            view.ctx.timeout(),
        ))
    })?;
    let results_ty: Vec<_> = results_ty.into_iter().collect();
    let paths = params_async_paths(&results_ty, resources);
    let rpc_name = rpc_func_name(&name);
    let start = Instant::now();
    let invocation = if let Some(timeout) = timeout {
        clt.timeout(timeout)
            .invoke(cx, &instance, rpc_name, buf, &paths)
            .await
    } else {
        clt.invoke(cx, &instance, rpc_name, buf, &paths).await
    };
    let (outgoing, incoming) = match invocation {
        Ok((outgoing, incoming)) => (outgoing, incoming),
        Err(err) => {
            return Ok(Err(err.context(format!(
                "failed to invoke `{instance}.{name}` polyfill via wRPC"
            ))));
        }
    };
    let tx = async {
        try_join_all(
            zip(0.., deferred)
                .filter_map(|(i, f)| f.map(|f| (outgoing.index(&[i]), f)))
                .map(|(w, f)| async move {
                    let w = w.map_err(wasmtime::Error::from)?;
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
        wasmtime::error::Ok(())
    };
    let rx = async {
        let mut incoming = pin!(incoming);
        for (i, (v, ref ty)) in zip(results, results_ty).enumerate() {
            read_value(&mut *store, &mut incoming, resources, v, ty, &[i])
                .await
                .with_context(|| format!("failed to decode return value {i}"))?;
        }
        wasmtime::error::Ok(())
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
        Err(err) => Ok(Err(err.into())),
    }
}

/// Polyfill [`types::ComponentFunc`] in a [`LinkerInstance`] using [`wrpc_transport::Invoke`]
///
/// Imports declared as `async func` are linked using [`LinkerInstance::func_new_concurrent`], all
/// other imports using [`LinkerInstance::func_new_async`].
/// The former only access the store through an [`Accessor`](wasmtime::component::Accessor), which
/// cannot be held across `await` points, therefore [`WrpcView::Invoke`] is cloned for each
/// invocation and the view must be `'static`.
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
    V: WrpcView<Invoke: Clone> + 'static,
{
    let span = Span::current();
    let instance = instance.into();
    let name = name.into();
    let host_resources = host_resources.into();
    let resources = ResourceTypes {
        guest: guest_resources.into(),
        #[cfg(feature = "wasmtime-wasi")]
        io_streams: Arc::from([ResourceType::host::<wasmtime_wasi::p2::DynInputStream>()]),
    };
    let result_ty = rpc_result_type(&host_resources, ty.results());
    if ty.async_() {
        linker.func_new_concurrent(&Arc::clone(&name), move |store, ty, params, results| {
            Box::pin(
                call(
                    store,
                    ty,
                    params,
                    results,
                    result_ty.clone(),
                    resources.clone(),
                    Arc::clone(&instance),
                    Arc::clone(&name),
                )
                .instrument(span.clone()),
            )
        })
    } else {
        linker.func_new_async(&Arc::clone(&name), move |store, ty, params, results| {
            Box::new(
                call(
                    store,
                    ty,
                    params,
                    results,
                    result_ty.clone(),
                    resources.clone(),
                    Arc::clone(&instance),
                    Arc::clone(&name),
                )
                .instrument(span.clone()),
            )
        })
    }
}

#[allow(clippy::too_many_arguments)]
async fn call(
    mut store: impl WithStore<Data: WrpcView<Invoke: Clone>>,
    ty: types::ComponentFunc,
    params: &[Val],
    results: &mut [Val],
    result_ty: Option<Option<Type>>,
    resources: ResourceTypes,
    instance: Arc<str>,
    name: Arc<str>,
) -> wasmtime::Result<()> {
    let Some(result_ty) = result_ty else {
        let res = invoke(
            &mut store,
            params,
            results,
            &resources,
            ty.params(),
            ty.results(),
            instance,
            name,
        )
        .await?;
        return res.map_err(wasmtime::Error::from_anyhow);
    };
    let [result] = results else {
        bail!("result type mismatch");
    };
    let mut ok = result_ty.as_ref().map(|_| Val::Bool(false));
    let res = invoke(
        &mut store,
        params,
        ok.as_mut_slice(),
        &resources,
        ty.params(),
        result_ty,
        instance,
        name,
    )
    .await?;
    *result = match res {
        Ok(()) => Val::Result(Ok(ok.map(Box::new))),
        Err(err) => store.with_store(|mut store| {
            let err = store.data_mut().push_error(Error::Invoke(err))?;
            let err = err
                .try_into_resource_any(&mut store)
                .context("failed to lower error resource")?;
            wasmtime::error::Ok(Val::Result(Err(Some(Box::new(Val::Resource(err))))))
        })?,
    };
    Ok(())
}
