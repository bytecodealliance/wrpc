//! wRPC TCP transport using [tokio]

use bytes::Bytes;
use tokio::io::BufReader;
use tokio::net::{TcpStream, ToSocketAddrs};
use tracing::instrument;

use crate::Invoke;
use crate::frame::{Incoming, Outgoing, invoke};

/// [Invoke] implementation of a TCP transport using [tokio]
#[derive(Clone, Debug)]
pub struct Client<T>(T);

impl<T> From<T> for Client<T>
where
    T: ToSocketAddrs + Clone,
{
    fn from(addr: T) -> Self {
        Self(addr)
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
        let stream = TcpStream::connect(self.0.clone()).await?;
        let (rx, tx) = stream.into_split();
        invoke(tx, BufReader::new(rx), instance, func, params, paths).await
    }
}
