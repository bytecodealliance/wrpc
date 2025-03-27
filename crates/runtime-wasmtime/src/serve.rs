use core::future::Future;
use core::pin::Pin;

use std::sync::Arc;

use anyhow::Context as _;
use futures::{Stream, TryStreamExt as _};
use tokio::sync::Mutex;
use tracing::{debug, instrument, Instrument as _, Span};
use wasmtime::component::types;
use wasmtime::component::{Instance, InstancePre, ResourceType};
use wasmtime::AsContextMut;
use wasmtime_wasi::WasiView;

use crate::{call, rpc_func_name, WrpcView};

pub trait ServeExt: wrpc_transport::Serve {
    /// Serve [`types::ComponentFunc`] from an [`InstancePre`] instantiating it on each call.
    /// This serving method does not support guest-exported resources.
    #[instrument(level = "trace", skip(self, store, instance_pre))]
    fn serve_function<T>(
        &self,
        store: impl Fn() -> wasmtime::Store<T> + Send + 'static,
        instance_pre: InstancePre<T>,
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
        T: WasiView + WrpcView + 'static,
    {
        let span = Span::current();
        async move {
            debug!(instance = instance_name, name, "serving function export");
            let component_ty = instance_pre.component();
            let idx = if instance_name.is_empty() {
                None
            } else {
                let (_, idx) = component_ty
                    .export_index(None, instance_name)
                    .with_context(|| format!("export `{instance_name}` not found"))?;
                Some(idx)
            };
            let (_, idx) = component_ty
                .export_index(idx.as_ref(), name)
                .with_context(|| format!("export `{name}` not found"))?;

            // TODO: set paths
            let invocations = self.serve(instance_name, rpc_func_name(name), []).await?;
            let name = Arc::<str>::from(name);
            let params_ty: Arc<[_]> = ty.params().map(|(_, ty)| ty).collect();
            let results_ty: Arc<[_]> = ty.results().collect();
            Ok(invocations.map_ok(move |(cx, tx, rx)| {
                let instance_pre = instance_pre.clone();
                let name = Arc::clone(&name);
                let params_ty = Arc::clone(&params_ty);
                let results_ty = Arc::clone(&results_ty);

                let mut store = store();
                (
                    cx,
                    Box::pin(
                        async move {
                            let instance = instance_pre
                                .instantiate_async(&mut store)
                                .await
                                .context("failed to instantiate component")?;
                            let func = instance
                                .get_func(&mut store, idx)
                                .with_context(|| format!("function export `{name}` not found"))?;
                            call(
                                &mut store,
                                rx,
                                tx,
                                params_ty.iter(),
                                results_ty.iter(),
                                func,
                                &[],
                            )
                            .await
                        }
                        .instrument(span.clone()),
                    ) as Pin<Box<dyn Future<Output = _> + Send + 'static>>,
                )
            }))
        }
    }

    /// Like [`Self::serve_function`], but with a shared `store` instance.
    /// This is required to allow for serving functions, which operate on guest-exported resources.
    #[instrument(level = "trace", skip(self, store, instance, guest_resources))]
    fn serve_function_shared<T>(
        &self,
        store: Arc<Mutex<wasmtime::Store<T>>>,
        instance: Instance,
        guest_resources: impl Into<Arc<[ResourceType]>>,
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
        T: WasiView + WrpcView + 'static,
    {
        let span = Span::current();
        let guest_resources = guest_resources.into();
        async move {
            let func = {
                let mut store = store.lock().await;
                let idx = if instance_name.is_empty() {
                    None
                } else {
                    let idx = instance
                        .get_export(store.as_context_mut(), None, instance_name)
                        .with_context(|| format!("export `{instance_name}` not found"))?;
                    Some(idx)
                };
                let idx = instance
                    .get_export(store.as_context_mut(), idx.as_ref(), name)
                    .with_context(|| format!("export `{name}` not found"))?;
                instance.get_func(store.as_context_mut(), idx)
            }
            .with_context(|| format!("function export `{name}` not found"))?;
            debug!(instance = instance_name, name, "serving function export");
            // TODO: set paths
            let invocations = self.serve(instance_name, rpc_func_name(name), []).await?;
            let params_ty: Arc<[_]> = ty.params().map(|(_, ty)| ty).collect();
            let results_ty: Arc<[_]> = ty.results().collect();
            let guest_resources = Arc::clone(&guest_resources);
            Ok(invocations.map_ok(move |(cx, tx, rx)| {
                let params_ty = Arc::clone(&params_ty);
                let results_ty = Arc::clone(&results_ty);
                let guest_resources = Arc::clone(&guest_resources);
                let store = Arc::clone(&store);
                (
                    cx,
                    Box::pin(
                        async move {
                            let mut store = store.lock().await;
                            call(
                                &mut *store,
                                rx,
                                tx,
                                params_ty.iter(),
                                results_ty.iter(),
                                func,
                                &guest_resources,
                            )
                            .await?;
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
