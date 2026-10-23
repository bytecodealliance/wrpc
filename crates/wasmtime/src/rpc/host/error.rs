use wasmtime::component::Resource;

use crate::bindings::rpc::error::{Error, Host, HostError};
use crate::rpc::WrpcRpcImpl;
use crate::{WrpcView, WrpcViewExt as _};

impl<T: WrpcView> Host for WrpcRpcImpl<T> {}

impl<T: WrpcView> HostError for WrpcRpcImpl<T> {
    fn to_debug_string(&mut self, error: Resource<Error>) -> wasmtime::Result<String> {
        let error = self.0.get_error(&error)?;
        Ok(format!("{error:#}"))
    }

    fn drop(&mut self, error: Resource<Error>) -> wasmtime::Result<()> {
        self.0.delete_error(error)?;
        Ok(())
    }
}
