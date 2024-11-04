//! wRPC WebTransport transport

use core::ops::{Deref, DerefMut};

use anyhow::Context as _;
use bytes::Bytes;
use quinn::VarInt;
use tracing::{debug, error, trace, warn};
use wrpc_transport::frame::{invoke, Accept, Incoming, Outgoing};
use wrpc_transport::Invoke;
use wtransport::{Connection, RecvStream, SendStream};

/// WebTransport server with graceful stream shutdown handling
pub type Server = wrpc_transport::Server<(), RecvStream, SendStream, ConnHandler>;

/// WebTransport wRPC client
#[derive(Clone, Debug)]
pub struct Client(Connection);

impl From<Connection> for Client {
    fn from(session: Connection) -> Self {
        Self(session)
    }
}

impl Deref for Client {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Client {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Graceful stream shutdown handler
pub struct ConnHandler;

impl wrpc_transport::frame::ConnHandler<RecvStream, SendStream> for ConnHandler {
    async fn on_ingress(mut rx: RecvStream, res: std::io::Result<()>) {
        if let Err(err) = res {
            error!(?err, "ingress failed");
        } else {
            debug!("ingress successfully complete");
        }
        if let Ok(code) = VarInt::from_u64(0x52e4a40fa8db) {
            if let Err(err) = rx.quic_stream_mut().stop(code) {
                debug!(?err, "failed to close incoming stream");
            }
        }
    }

    async fn on_egress(mut tx: SendStream, res: std::io::Result<()>) {
        if let Err(err) = res {
            error!(?err, "egress failed");
        } else {
            debug!("egress successfully complete");
        }
        match tx.quic_stream_mut().stopped().await {
            Ok(None) => {
                trace!("stream successfully closed")
            }
            Ok(Some(code)) => {
                if u64::from(code) == 0x52e4a40fa8db {
                    trace!("stream successfully closed")
                } else {
                    warn!(?code, "stream closed with code")
                }
            }
            Err(err) => {
                error!(?err, "failed to await stream close");
            }
        }
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
        let stream = self
            .0
            .open_bi()
            .await
            .context("failed to initialize parameter stream")?;
        let (tx, rx) = stream.await.context("failed to open parameter stream")?;
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
        let (tx, rx) = self
            .0
            .accept_bi()
            .await
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))?;
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
