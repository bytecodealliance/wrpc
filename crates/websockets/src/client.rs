use anyhow::Context as _;
use bytes::Bytes;
use tokio_websockets::ClientBuilder;
use tokio_websockets::resolver::{Gai, Resolver};
use tracing::instrument;

use wrpc_transport::Invoke;
use wrpc_transport::frame::invoke;

use crate::split;

/// [Invoke] implementation in terms of a [`ClientBuilder`].
#[repr(transparent)]
pub struct Client<'a, R: Resolver = Gai>(ClientBuilder<'a, R>);

impl<'a, R: Resolver> Client<'a, R> {
    /// Constructs a new [Client] from a preconfigured [`ClientBuilder`].
    #[must_use]
    pub fn from_builder(builder: ClientBuilder<'a, R>) -> Self {
        Self(builder)
    }
}

impl<'a, R: Resolver> From<ClientBuilder<'a, R>> for Client<'a, R> {
    fn from(builder: ClientBuilder<'a, R>) -> Self {
        Self::from_builder(builder)
    }
}

impl<R: Resolver + Send + Sync> Invoke for Client<'_, R> {
    type Context = ();

    #[instrument(level = "trace", skip(self, paths, params), fields(params = format!("{params:02x?}")))]
    async fn invoke<P>(
        &self,
        (): Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(
        wrpc_transport::frame::Outgoing,
        wrpc_transport::frame::Incoming,
    )>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        let (ws, _res) = self
            .0
            .connect()
            .await
            .context("failed to establish WebSocket connection")?;
        let (tx, rx) = split(ws);
        invoke(tx, rx, instance, func, params, paths).await
    }
}
