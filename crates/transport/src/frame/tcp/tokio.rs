//! wRPC TCP transport using [tokio]

use core::pin::Pin;
use core::task::{Context, Poll, ready};

use std::io::IoSlice;

use bytes::Bytes;
use tokio::io::{AsyncWrite, BufReader};
use tokio::net::{TcpStream, ToSocketAddrs, tcp};
use tracing::instrument;

use crate::Invoke;
use crate::frame::{Incoming, Outgoing, invoke};

/// Write half of a TCP stream, which, if dropped before being shut down, aborts the connection
/// once the read half is dropped too, signaling the peer that sending data failed
pub struct WriteHalf {
    inner: Option<tcp::OwnedWriteHalf>,
    shutdown: bool,
}

impl WriteHalf {
    fn inner(&mut self) -> std::io::Result<Pin<&mut tcp::OwnedWriteHalf>> {
        let inner = self
            .inner
            .as_mut()
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotConnected))?;
        Ok(Pin::new(inner))
    }
}

impl From<tcp::OwnedWriteHalf> for WriteHalf {
    fn from(inner: tcp::OwnedWriteHalf) -> Self {
        Self {
            inner: Some(inner),
            shutdown: false,
        }
    }
}

impl Drop for WriteHalf {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take()
            && !self.shutdown
        {
            _ = inner.as_ref().set_zero_linger();
            inner.forget();
        }
    }
}

impl AsyncWrite for WriteHalf {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.get_mut().inner()?.poll_write(cx, buf)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        self.get_mut().inner()?.poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner
            .as_ref()
            .is_some_and(AsyncWrite::is_write_vectored)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.get_mut().inner()?.poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        ready!(this.inner()?.poll_shutdown(cx))?;
        this.shutdown = true;
        Poll::Ready(Ok(()))
    }
}

/// [Invoke] implementation of a TCP transport using [tokio]
#[derive(Clone, Debug)]
pub struct Client<T> {
    addr: T,
    nodelay: Option<bool>,
    zero_linger: bool,
}

impl<T> Client<T> {
    /// Sets whether `TCP_NODELAY` is enabled on connections established by this client,
    /// `None` leaves the system default unchanged.
    /// It is enabled by default, since invocations consist of small writes that are each
    /// waited on by the peer.
    #[must_use]
    pub fn nodelay(self, nodelay: Option<bool>) -> Self {
        Self { nodelay, ..self }
    }

    /// Sets `SO_LINGER` to zero on connections established by this client, so that closing
    /// them aborts the connection instead of leaving it in `TIME_WAIT`.
    /// Aborting the connection discards any data not yet sent to the peer, so this should only be
    /// used if the peer does not depend on receiving all data, for example in benchmarks.
    /// It is not set by default.
    #[must_use]
    pub fn set_zero_linger(self) -> Self {
        Self {
            zero_linger: true,
            ..self
        }
    }
}

impl<T> From<T> for Client<T>
where
    T: ToSocketAddrs + Clone,
{
    fn from(addr: T) -> Self {
        Self {
            addr,
            nodelay: Some(true),
            zero_linger: false,
        }
    }
}

impl<T> Invoke for Client<T>
where
    T: ToSocketAddrs + Clone + Send + Sync,
{
    type Context = ();

    #[instrument(level = "trace", skip(self, paths, params), fields(params = format!("{params:02x?}")))]
    async fn invoke<P>(
        &self,
        (): Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Outgoing, Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        let stream = TcpStream::connect(self.addr.clone()).await?;
        if let Some(nodelay) = self.nodelay {
            stream.set_nodelay(nodelay)?;
        }
        if self.zero_linger {
            stream.set_zero_linger()?;
        }
        let (rx, tx) = stream.into_split();
        invoke(
            WriteHalf::from(tx),
            BufReader::new(rx),
            instance,
            func,
            params,
            paths,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;

    use super::*;

    #[test_log::test(tokio::test)]
    async fn write_half_dropped() {
        let lis = TcpListener::bind("[::1]:0").await.unwrap();
        let addr = lis.local_addr().unwrap();
        for shutdown in [true, false] {
            let (clt, srv) = tokio::join!(TcpStream::connect(addr), lis.accept());
            let (rx, tx) = clt.unwrap().into_split();
            let mut tx = WriteHalf::from(tx);
            tx.write_all(b"foo").await.unwrap();
            if shutdown {
                tx.shutdown().await.unwrap();
            }
            drop(tx);
            drop(rx);
            let (mut srv, ..) = srv.unwrap();
            let mut buf = Vec::default();
            let res = srv.read_to_end(&mut buf).await;
            if shutdown {
                res.unwrap();
                assert_eq!(buf, b"foo");
            } else {
                assert_eq!(res.unwrap_err().kind(), std::io::ErrorKind::ConnectionReset);
            }
        }
    }
}
