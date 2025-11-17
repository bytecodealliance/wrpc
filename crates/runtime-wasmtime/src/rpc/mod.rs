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
use wasmtime::component::{HasData, Linker};
use wasmtime_wasi::p2::Pollable;
use wrpc_transport::Invoke;

use crate::{bindings, WrpcView};

mod host;

/// Wrapper struct, for which [crate::bindings::wrpc::transport::transport::Host] is implemented
#[repr(transparent)]
pub struct WrpcRpcImpl<T>(pub T);

impl<T: 'static> HasData for WrpcRpcImpl<T> {
    type Data<'a> = WrpcRpcImpl<&'a mut T>;
}

pub fn add_to_linker<T>(linker: &mut Linker<T>) -> anyhow::Result<()>
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

/// RPC error
pub enum Error {
    /// Error originating from [Invoke::invoke] call
    Invoke(anyhow::Error),
    /// Error originating from [Index::index](wrpc_transport::Index::index) call on [Invoke::Incoming].
    IncomingIndex(anyhow::Error),
    /// Error originating from [Index::index](wrpc_transport::Index::index) call on
    /// [Invoke::Outgoing].
    OutgoingIndex(anyhow::Error),
    /// Error originating from a `wasi:io` stream provided by this crate.
    Stream(StreamError),
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Invoke(error) | Error::IncomingIndex(error) | Error::OutgoingIndex(error) => {
                error.fmt(f)
            }
            Error::Stream(error) => error.fmt(f),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Invoke(error) | Error::IncomingIndex(error) | Error::OutgoingIndex(error) => {
                error.fmt(f)
            }
            Error::Stream(error) => error.fmt(f),
        }
    }
}

/// Error type originating from `wasi:io` streams provided by this crate.
pub enum StreamError {
    LockPoisoned,
    TypeMismatch(&'static str),
    Read(std::io::Error),
    Write(std::io::Error),
    Flush(std::io::Error),
    Shutdown(std::io::Error),
}

impl core::error::Error for StreamError {}

impl fmt::Debug for StreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StreamError::LockPoisoned => "lock poisoned".fmt(f),
            StreamError::TypeMismatch(error) => error.fmt(f),
            StreamError::Read(error)
            | StreamError::Write(error)
            | StreamError::Flush(error)
            | StreamError::Shutdown(error) => error.fmt(f),
        }
    }
}

impl fmt::Display for StreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StreamError::LockPoisoned => "lock poisoned".fmt(f),
            StreamError::TypeMismatch(error) => error.fmt(f),
            StreamError::Read(error)
            | StreamError::Write(error)
            | StreamError::Flush(error)
            | StreamError::Shutdown(error) => error.fmt(f),
        }
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
                StreamError::LockPoisoned,
            )));
        };
        let Some(incoming) = incoming.downcast_mut::<T>() else {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                StreamError::TypeMismatch("invalid incoming channel type"),
            )));
        };
        Pin::new(incoming)
            .poll_read(cx, buf)
            .map_err(|err| std::io::Error::new(err.kind(), StreamError::Read(err)))
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
                StreamError::LockPoisoned,
            )));
        };
        let Some(outgoing) = outgoing.downcast_mut::<T>() else {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                StreamError::TypeMismatch("invalid outgoing channel type"),
            )));
        };
        Pin::new(outgoing)
            .poll_write(cx, buf)
            .map_err(|err| std::io::Error::new(err.kind(), StreamError::Write(err)))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        let Ok(mut outgoing) = self.outgoing.0.write() else {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::Deadlock,
                StreamError::LockPoisoned,
            )));
        };
        let Some(outgoing) = outgoing.downcast_mut::<T>() else {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                StreamError::TypeMismatch("invalid outgoing channel type"),
            )));
        };
        Pin::new(outgoing)
            .poll_flush(cx)
            .map_err(|err| std::io::Error::new(err.kind(), StreamError::Flush(err)))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        let Ok(mut outgoing) = self.outgoing.0.write() else {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::Deadlock,
                StreamError::LockPoisoned,
            )));
        };
        let Some(outgoing) = outgoing.downcast_mut::<T>() else {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                StreamError::TypeMismatch("invalid outgoing channel type"),
            )));
        };
        Pin::new(outgoing)
            .poll_shutdown(cx)
            .map_err(|err| std::io::Error::new(err.kind(), StreamError::Shutdown(err)))
    }
}
