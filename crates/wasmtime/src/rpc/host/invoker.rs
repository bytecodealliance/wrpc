use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use anyhow::anyhow;
use futures::FutureExt as _;
use tokio::join;
use wasmtime::component::{Access, FutureReader, Resource};
use wasmtime::error::Context as _;
use wrpc_introspect::merge_wildcards;
use wrpc_transport::{Invoke, InvokeExt as _};

use crate::bindings::rpc::context::Context;
use crate::bindings::rpc::invoker::{Host, HostWithStore};
use crate::bindings::rpc::transport::{IncomingChannel, OutgoingChannel};
use crate::rpc::host::ResultProducer;
use crate::rpc::{Error, IncomingRoot, WrpcRpcImpl};
use crate::{WrpcView, WrpcViewExt as _};

impl<T: WrpcView> Host for WrpcRpcImpl<T> {}

impl<T> HostWithStore<T> for WrpcRpcImpl<T>
where
    T: WrpcView + 'static,
    T::Invoke: Clone + 'static,
    <T::Invoke as Invoke>::Context: 'static,
{
    fn invoke(
        mut store: Access<'_, T, Self>,
        cx: Resource<Context>,
        instance: String,
        name: String,
        params: Vec<u8>,
        paths: Vec<Vec<Option<u32>>>,
    ) -> wasmtime::Result<(
        Resource<OutgoingChannel>,
        Resource<IncomingChannel>,
        FutureReader<Result<(), Resource<Error>>>,
    )> {
        let WrpcRpcImpl(wrpc) = store.get();
        let client = wrpc.wrpc().ctx.client().clone();
        let timeout = wrpc.wrpc().ctx.timeout();
        let cx = wrpc.delete_context(cx)?;
        let limit = wrpc.wrpc().ctx.max_path_indices();
        let n: usize = paths.iter().map(Vec::len).sum();
        let (paths, exceeded) = if n > limit {
            let err =
                anyhow!("invocation declares {n} path indices, more than the limit of {limit}");
            (Vec::default(), Some(err))
        } else {
            (paths, None)
        };
        let paths = paths
            .into_iter()
            .map(|path| {
                path.into_iter()
                    .map(|i| i.map(usize::try_from).transpose())
                    .collect::<Result<VecDeque<_>, _>>()
            })
            .collect::<Result<BTreeSet<_>, _>>()
            .context("failed to construct subscription paths")?;
        let paths: Box<[Box<[_]>]> = merge_wildcards(paths)
            .into_iter()
            .map(|path| Vec::from(path).into())
            .collect();
        let outgoing_root = Arc::<OnceLock<_>>::default();
        let incoming_root = Arc::<IncomingRoot>::default();
        let finished = Arc::<Mutex<Option<_>>>::default();
        let transmission = {
            let paths = paths.clone();
            let outgoing_root = Arc::downgrade(&outgoing_root);
            let incoming_root = Arc::downgrade(&incoming_root);
            let finished = Arc::downgrade(&finished);
            async move {
                if let Some(err) = exceeded {
                    return Err(Arc::new(err));
                }
                let invocation = if let Some(timeout) = timeout {
                    client
                        .timeout(timeout)
                        .invoke(cx, &instance, &name, params.into(), paths)
                        .await
                } else {
                    client
                        .invoke(cx, &instance, &name, params.into(), paths)
                        .await
                };
                let (outgoing, incoming) = invocation.map_err(Arc::new)?;
                if let Some(finished) = finished.upgrade() {
                    *finished.lock().unwrap_or_else(PoisonError::into_inner) =
                        Some((outgoing.finished().boxed(), incoming.finished().boxed()));
                }
                if let Some(root) = outgoing_root.upgrade() {
                    _ = root.set(outgoing);
                }
                if let Some(root) = incoming_root.upgrade() {
                    root.set(incoming);
                }
                Ok(())
            }
            .boxed()
            .shared()
        };
        let tx = OutgoingChannel::new(transmission.clone(), outgoing_root);
        let rx = IncomingChannel::new(transmission.clone(), incoming_root, &paths);
        let res = async move {
            if let Err(err) = transmission.await {
                return Err(Error::Transmission(err));
            }
            let Some((egress, ingress)) = finished
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take()
            else {
                return Err(Error::Invoke(anyhow!("invocation was not transmitted")));
            };
            match join!(egress, ingress) {
                (Err(err), ..) => Err(Error::Invoke(
                    anyhow::Error::new(err).context("failed to transmit data"),
                )),
                (.., Err(err)) => Err(Error::Invoke(
                    anyhow::Error::new(err).context("failed to receive data"),
                )),
                _ => Ok(()),
            }
        }
        .boxed();
        let table = wrpc.wrpc().table;
        let tx = table
            .push(tx)
            .context("failed to push outgoing channel to table")?;
        let rx = table
            .push(rx)
            .context("failed to push incoming channel to table")?;
        let res = FutureReader::new(&mut store, ResultProducer(res))?;
        Ok((tx, rx, res))
    }
}
