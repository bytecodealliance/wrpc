//! `wrpc:transport` implementation

use core::any::Any;
use core::fmt;
use core::marker::PhantomData;
use core::pin::Pin;
use core::task::{self, Poll};

use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

#[cfg(feature = "wasmtime-wasi")]
mod host;

#[cfg(feature = "wasmtime-wasi")]
pub use host::{WrpcRpcImpl, add_to_linker};

#[repr(transparent)]
pub struct Context(pub Box<dyn Any + Send>);

/// RPC error
pub enum Error {
    /// Error originating from [`Invoke::invoke`](wrpc_transport::Invoke::invoke) call
    Invoke(anyhow::Error),
    /// Error originating from an [`index`](wrpc_transport::frame::Incoming::index) call on the
    /// incoming framed stream.
    IncomingIndex(std::io::Error),
    /// Error originating from an [`index`](wrpc_transport::frame::Outgoing::index) call on the
    /// outgoing framed stream.
    OutgoingIndex(std::io::Error),
    /// Error originating from a `wasi:io` stream provided by this crate.
    Stream(StreamError),
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Invoke(error) => error.fmt(f),
            Error::IncomingIndex(error) | Error::OutgoingIndex(error) => error.fmt(f),
            Error::Stream(error) => error.fmt(f),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Invoke(error) => error.fmt(f),
            Error::IncomingIndex(error) | Error::OutgoingIndex(error) => error.fmt(f),
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

pub struct OutgoingChannel(pub Arc<std::sync::RwLock<Box<dyn Any + Send + Sync>>>);

pub struct IncomingChannel(pub Arc<std::sync::RwLock<Box<dyn Any + Send + Sync>>>);

pub struct IncomingChannelStream<T> {
    incoming: IncomingChannel,
    _ty: PhantomData<T>,
}

impl<T: AsyncRead + Unpin + 'static> AsyncRead for IncomingChannelStream<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
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
        cx: &mut task::Context<'_>,
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

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
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
            .poll_flush(cx)
            .map_err(|err| std::io::Error::new(err.kind(), StreamError::Flush(err)))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
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
