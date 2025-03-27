//! `wrpc:transport` implementation

use core::any::Any;
use core::fmt;
use core::future::Future;
use core::marker::PhantomData;
use core::pin::Pin;
use core::task::{Context, Poll};

use std::sync::Arc;

use anyhow::Context as _;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use wasmtime::component::Linker;
use wasmtime_wasi::Pollable;
use wrpc_transport::Invoke;

use crate::{bindings, WrpcView};

mod host;

/// Wrapper struct, for which [crate::bindings::wrpc::transport::transport::Host] is implemented
#[repr(transparent)]
pub struct WrpcRpcImpl<T>(pub T);

fn type_annotate<T, F>(val: F) -> F
where
    F: Fn(&mut T) -> WrpcRpcImpl<&mut T>,
{
    val
}

pub fn add_to_linker<T>(linker: &mut Linker<T>) -> anyhow::Result<()>
where
    T: WrpcView,
    T::Invoke: Clone + 'static,
    <T::Invoke as Invoke>::Context: 'static,
{
    let closure = type_annotate::<T, _>(|t| WrpcRpcImpl(t));
    bindings::rpc::context::add_to_linker_get_host(linker, closure)
        .context("failed to link `wrpc:rpc/context`")?;
    bindings::rpc::error::add_to_linker_get_host(linker, closure)
        .context("failed to link `wrpc:rpc/error`")?;
    bindings::rpc::invoker::add_to_linker_get_host(linker, closure)
        .context("failed to link `wrpc:rpc/invoker`")?;
    bindings::rpc::transport::add_to_linker_get_host(linker, closure)
        .context("failed to link `wrpc:rpc/transport`")?;
    Ok(())
}

#[repr(transparent)]
pub struct Error(pub anyhow::Error);

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

pub enum Invocation {
    Future(Pin<Box<dyn Future<Output = Box<dyn Any + Send>> + Send>>),
    Ready(Box<dyn Any + Send>),
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

pub struct OutgoingChannel(pub Arc<std::sync::RwLock<Box<dyn Any + Send + Sync>>>);

pub struct IncomingChannel(pub Arc<std::sync::RwLock<Box<dyn Any + Send + Sync>>>);

pub struct IncomingChannelStream<T> {
    incoming: IncomingChannel,
    _ty: PhantomData<T>,
}

impl<T: AsyncRead + Unpin + 'static> AsyncRead for IncomingChannelStream<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let Ok(mut incoming) = self.incoming.0.write() else {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::Deadlock,
                "lock poisoned",
            )));
        };
        let Some(incoming) = incoming.downcast_mut::<T>() else {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid incoming channel type",
            )));
        };
        Pin::new(incoming).poll_read(cx, buf)
    }
}

pub struct OutgoingChannelStream<T> {
    outgoing: OutgoingChannel,
    _ty: PhantomData<T>,
}

impl<T: AsyncWrite + Unpin + 'static> AsyncWrite for OutgoingChannelStream<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        let Ok(mut outgoing) = self.outgoing.0.write() else {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::Deadlock,
                "lock poisoned",
            )));
        };
        let Some(outgoing) = outgoing.downcast_mut::<T>() else {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid outgoing channel type",
            )));
        };
        Pin::new(outgoing).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        let Ok(mut outgoing) = self.outgoing.0.write() else {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::Deadlock,
                "lock poisoned",
            )));
        };
        let Some(outgoing) = outgoing.downcast_mut::<T>() else {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid outgoing channel type",
            )));
        };
        Pin::new(outgoing).poll_flush(cx)
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        let Ok(mut outgoing) = self.outgoing.0.write() else {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::Deadlock,
                "lock poisoned",
            )));
        };
        let Some(outgoing) = outgoing.downcast_mut::<T>() else {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid outgoing channel type",
            )));
        };
        Pin::new(outgoing).poll_shutdown(cx)
    }
}
