use core::{iter::zip, pin::pin};

use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _};
use tokio::io::AsyncWriteExt as _;
use tokio::try_join;
use tokio_util::bytes::BytesMut;
use tokio_util::codec::Encoder;
use tracing::{error, instrument, trace, warn};
use wasmtime::component::{types, Linker, Val};
use wasmtime::AsContextMut as _;
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiView};
use wit_parser::FunctionKind;
use wrpc_introspect::rpc_func_name;
use wrpc_runtime_wasmtime::{read_value, RemoteResource, ValEncoder};
use wrpc_transport::{Invocation, Session};
use wrpc_transport_next as wrpc_transport;

pub struct Ctx<C: wrpc_transport::Invoke<Error = wasmtime::Error>> {
    pub table: ResourceTable,
    pub wasi: WasiCtx,
    pub wrpc: C,
}

pub trait WrpcView<C: wrpc_transport::Invoke<Error = wasmtime::Error>>: Send {
    fn client(&self) -> &C;
}

impl<C: wrpc_transport::Invoke<Error = wasmtime::Error>> WrpcView<C> for Ctx<C> {
    fn client(&self) -> &C {
        &self.wrpc
    }
}

impl<C: wrpc_transport::Invoke<Error = wasmtime::Error>> WasiView for Ctx<C> {
    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.wasi
    }
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
}

/// Polyfills all missing imports
#[instrument(level = "trace", skip_all)]
pub fn polyfill<'a, T, C, V>(
    resolve: &wit_parser::Resolve,
    imports: T,
    engine: &wasmtime::Engine,
    ty: &types::Component,
    linker: &mut Linker<V>,
    cx: C::Context,
) where
    T: IntoIterator<Item = (&'a wit_parser::WorldKey, &'a wit_parser::WorldItem)>,
    T::IntoIter: ExactSizeIterator,
    V: WrpcView<C> + WasiView,
    C: wrpc_transport::Invoke<Error = wasmtime::Error>,
    C::Context: Clone + 'static,
    C::Incoming: Sized,
    C::Session: Session<TransportError = wasmtime::Error>,
{
    let imports = imports.into_iter();
    for (wk, item) in imports {
        let instance_name = resolve.name_world_key(wk);
        // Avoid polyfilling instances, for which static bindings are linked
        match instance_name.as_ref() {
            "wasi:cli/environment@0.2.0"
            | "wasi:cli/exit@0.2.0"
            | "wasi:cli/stderr@0.2.0"
            | "wasi:cli/stdin@0.2.0"
            | "wasi:cli/stdout@0.2.0"
            | "wasi:cli/terminal-input@0.2.0"
            | "wasi:cli/terminal-output@0.2.0"
            | "wasi:cli/terminal-stderr@0.2.0"
            | "wasi:cli/terminal-stdin@0.2.0"
            | "wasi:cli/terminal-stdout@0.2.0"
            | "wasi:clocks/monotonic-clock@0.2.0"
            | "wasi:clocks/wall-clock@0.2.0"
            | "wasi:filesystem/preopens@0.2.0"
            | "wasi:filesystem/types@0.2.0"
            | "wasi:http/incoming-handler@0.2.0"
            | "wasi:http/outgoing-handler@0.2.0"
            | "wasi:http/types@0.2.0"
            | "wasi:io/error@0.2.0"
            | "wasi:io/poll@0.2.0"
            | "wasi:io/streams@0.2.0"
            | "wasi:keyvalue/store@0.2.0-draft"
            | "wasi:random/random@0.2.0"
            | "wasi:sockets/instance-network@0.2.0"
            | "wasi:sockets/network@0.2.0"
            | "wasi:sockets/tcp-create-socket@0.2.0"
            | "wasi:sockets/tcp@0.2.0"
            | "wasi:sockets/udp-create-socket@0.2.0"
            | "wasi:sockets/udp@0.2.0" => continue,
            _ => {}
        }
        let wit_parser::WorldItem::Interface(interface) = item else {
            continue;
        };
        let Some(wit_parser::Interface { functions, .. }) = resolve.interfaces.get(*interface)
        else {
            warn!("component imports a non-existent interface");
            continue;
        };
        let Some(types::ComponentItem::ComponentInstance(instance)) =
            ty.get_import(engine, &instance_name)
        else {
            trace!(
                instance_name,
                "component does not import the parsed instance"
            );
            continue;
        };

        let mut linker = linker.root();
        let mut linker = match linker.instance(&instance_name) {
            Ok(linker) => linker,
            Err(err) => {
                error!(
                    ?err,
                    ?instance_name,
                    "failed to instantiate interface from root"
                );
                continue;
            }
        };
        let instance_name = Arc::new(instance_name);
        for (func_name, ty) in functions {
            trace!(
                ?instance_name,
                func_name,
                "polyfill component function import"
            );
            let Some(types::ComponentItem::ComponentFunc(func)) =
                instance.get_export(engine, func_name)
            else {
                trace!(
                    ?instance_name,
                    func_name,
                    "instance does not export the parsed function"
                );
                continue;
            };
            let cx = cx.clone();
            let instance_name = Arc::clone(&instance_name);
            let func_name = Arc::new(func_name.to_string());
            let rpc_name = Arc::new(rpc_func_name(ty).to_string());
            if let Err(err) = match ty.kind {
                FunctionKind::Method(..) => linker.func_new_async(
                    Arc::clone(&func_name).as_str(),
                    move |mut store, params, results| {
                        let cx = cx.clone();
                        let func = func.clone();
                        let rpc_name = Arc::clone(&rpc_name);
                        let func_name = Arc::clone(&func_name);
                        Box::new(async move {
                            let (this, params) = params.split_first().context("method receiver missing")?;
                            let Val::Resource(this) = this else {
                                bail!("first method parameter is not a resource")
                            };
                            let this = this
                                .try_into_resource(&mut store)
                                .context("resource type mismatch")?;
                            let mut buf = BytesMut::default();
                            for (v, ref ty) in zip(params, func.params()) {
                                ValEncoder::new(store.as_context_mut(), ty).encode(v, &mut buf).context("failed to encode parameter")?;
                            }
                            let RemoteResource(this) = store
                                .data_mut()
                                .table()
                                .get(&this)
                                .context("failed to get remote resource")?;
                            let this = this.to_string();
                            let Invocation { outgoing, incoming, session } = store
                                .data()
                                .client()
                                .invoke(cx, &this, &rpc_name, buf.freeze(), &[])
                                .await
                                .with_context(|| {
                                    format!("failed to invoke `{this}.{func_name}` polyfill via wRPC")
                                })?;
                            try_join!(
                                async {
                                    pin!(outgoing).shutdown().await.context("failed to shutdown outgoing stream")
                                },
                                async {
                                    let mut incoming = pin!(incoming);
                                    for (v, ref ty) in zip(results, func.results()) {
                                        read_value(&mut store, &mut incoming, v, ty).await.context("failed to decode result value")?;
                                    }
                                    Ok(())
                                },
                            )?;
                            match session.finish(Ok(())).await? {
                                Ok(()) => Ok(()),
                                Err(err) => bail!(anyhow!("{err}").context("session failed"))
                            }
                        })
                    },
                ),
                FunctionKind::Freestanding | FunctionKind::Static(_) | FunctionKind::Constructor(..) => linker.func_new_async(
                    Arc::clone(&func_name).as_str(),
                    move |mut store, params, results| {
                        let cx = cx.clone();
                        let instance_name = Arc::clone(&instance_name);
                        let func = func.clone();
                        let rpc_name = Arc::clone(&rpc_name);
                        let func_name = Arc::clone(&func_name);
                        Box::new(async move {
                            let mut buf = BytesMut::default();
                            for (v, ref ty) in zip(params, func.params()) {
                                ValEncoder::new(store.as_context_mut(), ty).encode(v, &mut buf).context("failed to encode parameter")?;
                            }
                            let Invocation { outgoing, incoming, session } = store
                                .data()
                                .client()
                                .invoke(cx, &instance_name, &rpc_name, buf.freeze(), &[])
                                .await
                                .with_context(|| {
                                    format!("failed to invoke `{instance_name}.{func_name}` polyfill via wRPC")
                                })?;
                            try_join!(
                                async {
                                    pin!(outgoing).shutdown().await.context("failed to shutdown outgoing stream")
                                },
                                async {
                                    let mut incoming = pin!(incoming);
                                    for (v, ref ty) in zip(results, func.results()) {
                                        read_value(&mut store, &mut incoming, v, ty).await.context("failed to decode result value")?;
                                    }
                                    Ok(())
                                },
                            )?;
                            match session.finish(Ok(())).await? {
                                Ok(()) => Ok(()),
                                Err(err) => bail!(anyhow!("{err}").context("session failed"))
                            }
                        })
                    },
                ),
            } {
                error!(?err, "failed to polyfill component function import");
            }
        }
    }
}
