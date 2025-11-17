use wasmtime::component::Resource;
use wrpc_transport::Invoke;

use crate::bindings::rpc::context::{Context, Host, HostContext};
use crate::rpc::WrpcRpcImpl;
use crate::{WrpcView, WrpcViewExt as _};

impl<T: WrpcView> Host for WrpcRpcImpl<T> where <T::Invoke as Invoke>::Context: 'static {}

impl<T: WrpcView> HostContext for WrpcRpcImpl<T>
where
    <T::Invoke as Invoke>::Context: 'static,
{
    fn default(&mut self) -> wasmtime::Result<Resource<Context>> {
        let cx = self.0.wrpc().ctx.context();
        self.0.push_context(cx)
    }

    fn drop(&mut self, cx: Resource<Context>) -> wasmtime::Result<()> {
        self.0.delete_context(cx)?;
        Ok(())
    }
}
