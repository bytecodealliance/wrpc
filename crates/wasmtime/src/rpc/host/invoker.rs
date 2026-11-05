use std::collections::{BTreeSet, VecDeque};

use anyhow::anyhow;
use wasmtime::component::{Accessor, Resource};
use wasmtime::error::Context as _;
use wrpc_introspect::merge_wildcards;
use wrpc_transport::{Invoke, InvokeExt as _};

use crate::bindings::rpc::context::Context;
use crate::bindings::rpc::invoker::{Host, HostWithStore};
use crate::bindings::rpc::transport::{IncomingChannel, OutgoingChannel};
use crate::rpc::{Error, WrpcRpcImpl};
use crate::{WrpcView, WrpcViewExt as _};

impl<T: WrpcView> Host for WrpcRpcImpl<T> {}

impl<T> HostWithStore<T> for WrpcRpcImpl<T>
where
    T: WrpcView + 'static,
    T::Invoke: Clone + 'static,
    <T::Invoke as Invoke>::Context: 'static,
{
    async fn invoke(
        store: &Accessor<T, Self>,
        cx: Resource<Context>,
        instance: String,
        name: String,
        params: Vec<u8>,
        paths: Vec<Vec<Option<u32>>>,
    ) -> wasmtime::Result<
        Result<(Resource<OutgoingChannel>, Resource<IncomingChannel>), Resource<Error>>,
    > {
        let (client, timeout, cx, limit) = store.with(|mut store| {
            let WrpcRpcImpl(wrpc) = store.get();
            let client = wrpc.wrpc().ctx.client().clone();
            let timeout = wrpc.wrpc().ctx.timeout();
            let cx = wrpc.delete_context(cx)?;
            let limit = wrpc.wrpc().ctx.max_path_indices();
            wasmtime::error::Ok((client, timeout, cx, limit))
        })?;
        let n: usize = paths.iter().map(Vec::len).sum();
        if n > limit {
            return store.with(|mut store| {
                let WrpcRpcImpl(wrpc) = store.get();
                let err = wrpc.push_error(Error::Invoke(anyhow!(
                    "invocation declares {n} path indices, more than the limit of {limit}"
                )))?;
                Ok(Err(err))
            });
        }
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
        let invocation = if let Some(timeout) = timeout {
            client
                .timeout(timeout)
                .invoke_framed(cx, &instance, &name, params.into(), paths)
                .await
        } else {
            client
                .invoke_framed(cx, &instance, &name, params.into(), paths)
                .await
        };
        store.with(|mut store| {
            let WrpcRpcImpl(wrpc) = store.get();
            let (outgoing, incoming) = match invocation {
                Ok(invocation) => invocation,
                Err(err) => {
                    let err = wrpc.push_error(Error::Invoke(err))?;
                    return Ok(Err(err));
                }
            };
            let table = wrpc.wrpc().table;
            let tx = table
                .push(OutgoingChannel::new(Box::new(outgoing)))
                .context("failed to push outgoing channel to table")?;
            let rx = table
                .push(IncomingChannel::new(Box::new(incoming)))
                .context("failed to push incoming channel to table")?;
            Ok(Ok((tx, rx)))
        })
    }
}
