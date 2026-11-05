//! wRPC TCP transport using [tokio]

use core::net::SocketAddr;

use bytes::Bytes;
use tokio::io::BufReader;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};
use tracing::instrument;

use crate::Invoke;
use crate::frame::{AcceptError, invoke};

/// TCP server
pub type Server = crate::frame::Server<SocketAddr, BufReader<OwnedReadHalf>, OwnedWriteHalf>;

/// Accepts a connection on `lis` and an invocation on it with the peer address as the context,
/// see [`Server::accept`].
///
/// # Errors
///
/// Returns an error if accepting the connection or the invocation fails
pub async fn accept(
    srv: &Server,
    lis: &TcpListener,
) -> Result<(), AcceptError<SocketAddr, BufReader<OwnedReadHalf>, OwnedWriteHalf>> {
    let (stream, addr) = lis.accept().await.map_err(AcceptError::IO)?;
    let (rx, tx) = stream.into_split();
    srv.accept(addr, tx, BufReader::new(rx)).await
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
    type Outgoing = OwnedWriteHalf;
    type Incoming = BufReader<OwnedReadHalf>;

    #[instrument(level = "trace", skip(self, _paths, params), fields(params = format!("{params:02x?}")))]
    async fn invoke<P>(
        &self,
        (): Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        _paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Self::Outgoing, Self::Incoming)>
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
        invoke(tx, BufReader::new(rx), instance, func, params).await
    }
}
