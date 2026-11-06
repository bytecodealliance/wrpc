//! `wrpc:rpc` implementation

use core::fmt;
use core::pin::Pin;
use core::task::{Context, Waker};

use std::collections::BTreeSet;
use std::sync::Arc;

use futures::FutureExt as _;
use futures::future::BoxFuture;
use tokio::io::{AsyncBufRead, AsyncRead, AsyncWrite};
use wasmtime::bail;
use wasmtime::component::{HasData, Linker};
use wasmtime::error::Context as _;
use wrpc_transport::Invoke;
use wrpc_transport::frame::{Incoming, Outgoing};

use crate::{WrpcView, bindings};

mod host;

/// Wrapper struct, for which [`crate::bindings::rpc::transport::Host`] is implemented
#[repr(transparent)]
pub struct WrpcRpcImpl<T>(pub T);

impl<T: 'static> HasData for WrpcRpcImpl<T> {
    type Data<'a> = WrpcRpcImpl<&'a mut T>;
}

pub fn add_to_linker<T>(linker: &mut Linker<T>) -> wasmtime::Result<()>
where
    T: WrpcView + 'static,
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
    /// Failure of an invocation
    Invoke(anyhow::Error),
    /// Error originating from reading the data of an incoming channel
    Read(std::io::Error),
    /// Error originating from writing the data of an outgoing channel
    Write(std::io::Error),
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Invoke(error) => error.fmt(f),
            Error::Read(error) | Error::Write(error) => error.fmt(f),
        }
    }
}

/// [Incoming] of a channel, which erases the stream type of the transport
trait DynIncoming: AsyncRead + Send + Unpin {
    fn index(&self, path: &[usize]) -> std::io::Result<Box<dyn DynIncoming>>;
    fn closed(&self) -> BoxFuture<'static, std::io::Result<()>>;
}

impl<T: AsyncBufRead + Send + Unpin + 'static> DynIncoming for Incoming<T> {
    fn index(&self, path: &[usize]) -> std::io::Result<Box<dyn DynIncoming>> {
        let incoming = Self::index(self, path)?;
        Ok(Box::new(incoming))
    }

    fn closed(&self) -> BoxFuture<'static, std::io::Result<()>> {
        Self::closed(self).boxed()
    }
}

/// [Outgoing] of a channel, which erases the stream type of the transport
trait DynOutgoing: AsyncWrite + Send + Unpin {
    fn index(&self, path: &[usize]) -> std::io::Result<Box<dyn DynOutgoing>>;
}

impl<T: AsyncWrite + Send + Unpin + 'static> DynOutgoing for Outgoing<T> {
    fn index(&self, path: &[usize]) -> std::io::Result<Box<dyn DynOutgoing>> {
        let outgoing = Self::index(self, path)?;
        Ok(Box::new(outgoing))
    }
}

/// `wrpc:rpc/transport.incoming-channel` representation
pub struct IncomingChannel {
    /// Handle reading the data of the position, shared with the stream reading it
    incoming: Arc<std::sync::Mutex<Box<dyn DynIncoming>>>,
    data_taken: bool,
}

impl IncomingChannel {
    fn new(incoming: Box<dyn DynIncoming>) -> Self {
        Self {
            incoming: Arc::new(std::sync::Mutex::new(incoming)),
            data_taken: false,
        }
    }

    /// Returns the channel for the position at `path`
    fn index(&self, path: &[usize]) -> std::io::Result<Self> {
        let incoming = self
            .incoming
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .index(path)?;
        Ok(Self::new(incoming))
    }

    /// Takes the data of this channel, returning the handle reading it
    fn take_data(&mut self) -> wasmtime::Result<Arc<std::sync::Mutex<Box<dyn DynIncoming>>>> {
        if self.data_taken {
            bail!("channel data already taken");
        }
        self.data_taken = true;
        Ok(Arc::clone(&self.incoming))
    }
}

/// `wrpc:rpc/transport.outgoing-channel` representation
pub struct OutgoingChannel {
    /// Handle writing the data of the position, shared with the stream writing it
    outgoing: Arc<std::sync::Mutex<Box<dyn DynOutgoing>>>,
    /// Positions, which channels were returned for, shared by the channels of an invocation
    returned: Arc<std::sync::Mutex<BTreeSet<Box<[usize]>>>>,
    path: Box<[usize]>,
    data_taken: bool,
}

impl OutgoingChannel {
    fn new(outgoing: Box<dyn DynOutgoing>) -> Self {
        Self {
            outgoing: Arc::new(std::sync::Mutex::new(outgoing)),
            returned: Arc::default(),
            path: Box::default(),
            data_taken: false,
        }
    }

    /// Returns the channel for the position at `path`
    fn index(&self, path: &[usize]) -> std::io::Result<Self> {
        let outgoing = self
            .outgoing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .index(path)?;
        let path: Box<[usize]> = [&self.path, path].concat().into();
        let mut returned = self
            .returned
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !returned.insert(path.clone()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("`{path:?}` channel already returned"),
            ));
        }
        drop(returned);
        Ok(Self {
            outgoing: Arc::new(std::sync::Mutex::new(outgoing)),
            returned: Arc::clone(&self.returned),
            path,
            data_taken: false,
        })
    }

    /// Takes the data of this channel, returning the handle writing it
    fn take_data(&mut self) -> wasmtime::Result<Arc<std::sync::Mutex<Box<dyn DynOutgoing>>>> {
        if self.data_taken {
            bail!("channel data already taken");
        }
        self.data_taken = true;
        Ok(Arc::clone(&self.outgoing))
    }
}

impl Drop for OutgoingChannel {
    fn drop(&mut self) {
        if !self.data_taken {
            let mut outgoing = self
                .outgoing
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            _ = Pin::new(&mut **outgoing).poll_shutdown(&mut Context::from_waker(Waker::noop()));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;

    use bytes::Bytes;
    use wrpc_transport::InvokeExt as _;
    use wrpc_transport::frame::Oneshot;

    use super::*;

    #[test_log::test(tokio::test)]
    async fn incoming_channel_index() {
        fn index_err(rx: &IncomingChannel, path: &[usize]) -> ErrorKind {
            let Err(err) = rx.index(path) else {
                panic!("indexing `{path:?}` should fail");
            };
            err.kind()
        }

        let (clt, _srv) = Oneshot::duplex(1024);
        let (.., rx) = clt
            .invoke_framed(
                (),
                "foo",
                "bar",
                Bytes::default(),
                [&[Some(0)][..], &[Some(1), None, Some(0)]],
            )
            .await
            .unwrap();
        let rx = IncomingChannel::new(Box::new(rx));
        let mut sub = rx.index(&[0]).unwrap();
        assert!(sub.take_data().is_ok());
        assert!(sub.take_data().is_err());
        assert_eq!(index_err(&rx, &[0]), ErrorKind::AlreadyExists);
        let prefix = rx.index(&[1]).unwrap();
        assert!(prefix.index(&[5, 0]).unwrap().take_data().is_ok());
        for path in [&[2][..], &[0, 0], &[1, 5, 1]] {
            assert_eq!(index_err(&rx, path), ErrorKind::NotFound);
        }
    }
}
