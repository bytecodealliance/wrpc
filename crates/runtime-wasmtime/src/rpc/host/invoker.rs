use anyhow::Context as _;
use tokio::io::AsyncWriteExt as _;
use wasmtime::component::Resource;
use wrpc_transport::Invoke;

use crate::bindings::rpc::context::Context;
use crate::bindings::rpc::invoker::Host;
use crate::bindings::rpc::transport::Invocation;
use crate::rpc::WrpcRpcImpl;
use crate::{WrpcView, WrpcViewExt as _};

impl<T: WrpcView> Host for WrpcRpcImpl<T>
where
    T::Invoke: Clone + 'static,
    <T::Invoke as Invoke>::Context: 'static,
{
    fn invoke(
        &mut self,
        cx: Resource<Context>,
        instance: String,
        name: String,
        params: Vec<u8>,
        paths: Vec<Vec<Option<u32>>>,
    ) -> wasmtime::Result<Resource<Invocation>> {
        let client = self.0.wrpc().ctx.client().clone();
        let cx = self.0.delete_context(cx)?;
        let paths = paths
            .into_iter()
            .map(|path| {
                path.into_iter()
                    .map(|i| i.map(usize::try_from).transpose())
                    .collect::<Result<Box<[_]>, _>>()
            })
            .collect::<Result<Box<[_]>, _>>()
            .context("failed to construct subscription paths")?;
        let invocation = async move {
            let (mut tx, rx) = client
                .invoke(cx, &instance, &name, params.into(), paths)
                .await?;
            tx.flush()
                .await
                .context("failed to flush outgoing stream")?;
            Ok((tx, rx))
        };
        self.0.push_invocation(invocation)
    }
}
