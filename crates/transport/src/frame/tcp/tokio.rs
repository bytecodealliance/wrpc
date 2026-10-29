//! wRPC TCP transport using [tokio]

use core::pin::Pin;
use core::task::{Context, Poll, ready};

use std::io::IoSlice;
use std::sync::{Arc, Mutex, PoisonError};

use bytes::Bytes;
use futures::task::AtomicWaker;
use tokio::io::{AsyncRead, AsyncWrite, BufReader, ReadBuf};
use tokio::net::{TcpStream, ToSocketAddrs};
use tracing::instrument;

use crate::Invoke;
use crate::frame::{Incoming, Outgoing, invoke};

/// Splits a TCP stream into a [ReadHalf] and a [WriteHalf].
///
/// If the [WriteHalf] is dropped before being shut down, the connection is aborted, signaling the
/// peer that sending data failed, and reading from the [ReadHalf] fails, unless it already
/// reached the end of the stream.
#[must_use]
pub fn split(stream: TcpStream) -> (ReadHalf, WriteHalf) {
    let shared = Arc::new(Shared {
        stream: Mutex::new(Some(stream)),
        waker: AtomicWaker::default(),
    });
    (
        ReadHalf {
            shared: Arc::clone(&shared),
            eof: false,
        },
        WriteHalf {
            shared,
            shutdown: false,
        },
    )
}

#[derive(Debug)]
struct Shared {
    stream: Mutex<Option<TcpStream>>,
    waker: AtomicWaker,
}

impl Shared {
    fn poll<T>(
        &self,
        f: impl FnOnce(Pin<&mut TcpStream>) -> Poll<std::io::Result<T>>,
    ) -> Poll<std::io::Result<T>> {
        let mut stream = self.stream.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(stream) = stream.as_mut() else {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                "TCP write half dropped before being shut down",
            )));
        };
        f(Pin::new(stream))
    }
}

/// Read half of a TCP stream returned by [split]
#[derive(Debug)]
pub struct ReadHalf {
    shared: Arc<Shared>,
    eof: bool,
}

impl AsyncRead for ReadHalf {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        if this.eof {
            return Poll::Ready(Ok(()));
        }
        this.shared.waker.register(cx.waker());
        let n = buf.filled().len();
        ready!(this.shared.poll(|stream| stream.poll_read(cx, buf)))?;
        this.eof = buf.filled().len() == n && buf.remaining() > 0;
        Poll::Ready(Ok(()))
    }
}

/// Write half of a TCP stream returned by [split], which, if dropped before being shut down,
/// aborts the connection, signaling the peer that sending data failed
#[derive(Debug)]
pub struct WriteHalf {
    shared: Arc<Shared>,
    shutdown: bool,
}

impl Drop for WriteHalf {
    fn drop(&mut self) {
        if self.shutdown {
            return;
        }
        let stream = self
            .shared
            .stream
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(stream) = stream {
            _ = stream.set_zero_linger();
        }
        self.shared.waker.wake();
    }
}

impl AsyncWrite for WriteHalf {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.shared.poll(|stream| stream.poll_write(cx, buf))
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        self.shared
            .poll(|stream| stream.poll_write_vectored(cx, bufs))
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.shared.poll(|stream| stream.poll_flush(cx))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        ready!(this.shared.poll(|stream| stream.poll_shutdown(cx)))?;
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
        let (rx, tx) = split(stream);
        invoke(tx, BufReader::new(rx), instance, func, params, paths).await
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
            let (mut rx, mut tx) = split(clt.unwrap());
            tx.write_all(b"foo").await.unwrap();
            if shutdown {
                tx.shutdown().await.unwrap();
            }
            drop(tx);
            let (mut srv, ..) = srv.unwrap();
            let mut buf = Vec::default();
            let res = srv.read_to_end(&mut buf).await;
            if shutdown {
                res.unwrap();
                assert_eq!(buf, b"foo");
            } else {
                assert_eq!(res.unwrap_err().kind(), std::io::ErrorKind::ConnectionReset);
                let err = rx.read_to_end(&mut Vec::default()).await.unwrap_err();
                assert_eq!(err.kind(), std::io::ErrorKind::ConnectionAborted);
            }
        }
    }
}
