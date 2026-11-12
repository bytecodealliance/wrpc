//! wRPC TCP transport using [tokio]

use core::borrow::Borrow;
use core::net::SocketAddr;
use core::task::{Poll, ready};

use futures::{Stream, stream};
use tokio::io::BufReader;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};
use tracing::{instrument, warn};

use crate::Connect;

/// Returns the stream of invocations accepted on `lis`, each carried by one connection, with
/// the peer address as the context
///
/// `TCP_NODELAY` is enabled on accepted connections, since invocations consist of small
/// writes that are each waited on by the peer.
pub fn incoming(
    lis: impl Borrow<TcpListener>,
) -> impl Stream<Item = std::io::Result<(SocketAddr, OwnedWriteHalf, BufReader<OwnedReadHalf>)>> {
    stream::poll_fn(move |cx| {
        let (stream, addr) = ready!(lis.borrow().poll_accept(cx))?;
        if let Err(err) = stream.set_nodelay(true) {
            warn!(?err, "failed to set `TCP_NODELAY`");
        }
        let (rx, tx) = stream.into_split();
        Poll::Ready(Some(Ok((addr, tx, BufReader::new(rx)))))
    })
}

/// [Connect] implementation of a TCP transport using [tokio]
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

impl<T> Connect for Client<T>
where
    T: ToSocketAddrs + Clone + Send + Sync,
{
    type Context = ();
    type Outgoing = OwnedWriteHalf;
    type Incoming = BufReader<OwnedReadHalf>;
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self))]
    async fn connect(
        &self,
        (): Self::Context,
    ) -> std::io::Result<(Self::Outgoing, Self::Incoming)> {
        let stream = TcpStream::connect(self.addr.clone()).await?;
        if let Some(nodelay) = self.nodelay {
            stream.set_nodelay(nodelay)?;
        }
        if self.zero_linger {
            stream.set_zero_linger()?;
        }
        let (rx, tx) = stream.into_split();
        Ok((tx, BufReader::new(rx)))
    }
}
