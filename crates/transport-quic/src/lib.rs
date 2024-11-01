//! wRPC QUIC transport

use anyhow::Context as _;
use bytes::Bytes;
use quinn::{Connection, RecvStream, SendStream};
use wrpc_transport::frame::{invoke, Accept, Incoming, Outgoing};
use wrpc_transport::Invoke;

/// QUIC transport client
#[derive(Clone, Debug)]
pub struct Client(Connection);

impl From<Connection> for Client {
    fn from(conn: Connection) -> Self {
        Self(conn)
    }
}

impl Invoke for &Client {
    type Context = ();
    type Outgoing = Outgoing;
    type Incoming = Incoming;

    async fn invoke<P>(
        &self,
        (): Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Self::Outgoing, Self::Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        let (tx, rx) = self
            .0
            .open_bi()
            .await
            .context("failed to open parameter stream")?;
        invoke(tx, rx, instance, func, params, paths).await
    }
}

impl Invoke for Client {
    type Context = ();
    type Outgoing = Outgoing;
    type Incoming = Incoming;

    async fn invoke<P>(
        &self,
        (): Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Self::Outgoing, Self::Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        (&self).invoke((), instance, func, params, paths).await
    }
}

impl Accept for &Client {
    type Context = ();
    type Outgoing = SendStream;
    type Incoming = RecvStream;

    async fn accept(&self) -> std::io::Result<(Self::Context, Self::Outgoing, Self::Incoming)> {
        let (tx, rx) = self.0.accept_bi().await?;
        Ok(((), tx, rx))
    }
}

impl Accept for Client {
    type Context = ();
    type Outgoing = SendStream;
    type Incoming = RecvStream;

    async fn accept(&self) -> std::io::Result<(Self::Context, Self::Outgoing, Self::Incoming)> {
        (&self).accept().await
    }
}
