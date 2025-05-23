use anyhow::Context as _;
use wasmtime::component::Resource;
use wasmtime_wasi::p2::bindings::io::error::Error as IoError;

use crate::bindings::rpc::error::{Error, Host, HostError};
use crate::rpc::WrpcRpcImpl;
use crate::{WrpcView, WrpcViewExt as _};

impl<T: WrpcView> Host for WrpcRpcImpl<T> {}

impl<T: WrpcView> HostError for WrpcRpcImpl<T> {
    fn from_io_error(
        &mut self,
        error: Resource<IoError>,
    ) -> wasmtime::Result<Result<Resource<Error>, Resource<IoError>>> {
        let table = self.0.table();
        let error = table
            .delete::<IoError>(error)
            .context("failed to delete `wasi:io/error.error` from table")?;
        match error.downcast() {
            Ok(error) => {
                let error = self.0.push_error(Error::Stream(error))?;
                Ok(Ok(error))
            }
            Err(error) => {
                let error = table
                    .push(error)
                    .context("failed to push `wasi:io/error.error` to table")?;
                Ok(Err(error))
            }
        }
    }

    fn to_debug_string(&mut self, error: Resource<Error>) -> wasmtime::Result<String> {
        let error = self.0.get_error(&error)?;
        Ok(format!("{error:#}"))
    }

    fn drop(&mut self, error: Resource<Error>) -> wasmtime::Result<()> {
        self.0.delete_error(error)?;
        Ok(())
    }
}
