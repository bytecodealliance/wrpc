use wasmtime::component::{HasData, Linker};
use wasmtime::error::Context as _;
use wasmtime_wasi::p2::Pollable;
use wrpc_transport::Invoke;

use crate::rpc::Invocation;
use crate::{WrpcView, bindings};

mod context;
mod error;
mod invoker;
mod transport;

/// Wrapper struct, for which [`crate::bindings::rpc::transport::Host`] is implemented
#[repr(transparent)]
pub struct WrpcRpcImpl<T>(pub T);

impl<T: 'static> HasData for WrpcRpcImpl<T> {
    type Data<'a> = WrpcRpcImpl<&'a mut T>;
}

pub fn add_to_linker<T>(linker: &mut Linker<T>) -> wasmtime::Result<()>
where
    T: WrpcView,
    T::Invoke: Clone + 'static,
    <T::Invoke as Invoke>::Context: 'static,
{
    bindings::rpc::context::add_to_linker::<_, WrpcRpcImpl<T>>(linker, |t| WrpcRpcImpl(t))
        .context("failed to link `wrpc:rpc/context`")?;
    bindings::rpc::error::add_to_linker::<_, WrpcRpcImpl<T>>(linker, |t| WrpcRpcImpl(t))
        .context("failed to link `wrpc:rpc/error`")?;
    bindings::rpc::invoker::add_to_linker::<_, WrpcRpcImpl<T>>(linker, |t| WrpcRpcImpl(t))
        .context("failed to link `wrpc:rpc/invoker`")?;
    bindings::rpc::transport::add_to_linker::<_, WrpcRpcImpl<T>>(linker, |t| WrpcRpcImpl(t))
        .context("failed to link `wrpc:rpc/transport`")?;
    Ok(())
}

#[wasmtime_wasi::async_trait]
impl Pollable for Invocation {
    async fn ready(&mut self) {
        match self {
            Self::Future(fut) => {
                let res = fut.await;
                *self = Self::Ready(res);
            }
            Self::Ready(..) => {}
        }
    }
}
