//! wRPC transport stream framing

use core::future::Future;

use bytes::Bytes;
use tokio::io::{duplex, split, AsyncRead, AsyncWrite, DuplexStream, ReadHalf, WriteHalf};
use tracing::instrument;

use crate::frame::{invoke, Incoming, Outgoing};
use crate::{Accept, Invoke};

/// [Invoke] and [Accept] implementation in terms of a single stream pair.
///
/// Either [`Invoke::invoke`] or [`Accept::accept`] can only be called at most once
/// on [Oneshot], repeated calls with return an error
#[derive(Debug)]
pub struct Oneshot<I, O>(std::sync::Mutex<Option<(I, O)>>);

impl<I, O> From<(I, O)> for Oneshot<I, O> {
    fn from((rx, tx): (I, O)) -> Self {
        Self(std::sync::Mutex::new(Some((rx, tx))))
    }
}

impl From<DuplexStream> for Oneshot<ReadHalf<DuplexStream>, WriteHalf<DuplexStream>> {
    fn from(stream: DuplexStream) -> Self {
        split(stream).into()
    }
}

impl Oneshot<ReadHalf<DuplexStream>, WriteHalf<DuplexStream>> {
    /// Creates a pair of connected [Oneshot] using [tokio::io::duplex].
    pub fn duplex(max_buf_size: usize) -> (Self, Self) {
        let (a, b) = duplex(max_buf_size);
        (a.into(), b.into())
    }
}

impl<I, O> Oneshot<I, O> {
    /// Returns the inner stream pair if [Oneshot] has not been used yet or an error.
    pub fn try_take_inner(&self) -> std::io::Result<(I, O)> {
        match self.0.try_lock().map(|mut stream| stream.take()) {
            Ok(Some((rx, tx))) => Ok((rx, tx)),
            Ok(None) | Err(std::sync::TryLockError::WouldBlock) => Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "stream was already used",
            )),
            Err(std::sync::TryLockError::Poisoned(..)) => {
                Err(std::io::Error::other("stream lock poisoned"))
            }
        }
    }
}

impl<I, O> Invoke for Oneshot<I, O>
where
    I: AsyncRead + Send + Unpin + 'static,
    O: AsyncWrite + Send + Unpin + 'static,
{
    type Context = ();
    type Outgoing = Outgoing;
    type Incoming = Incoming;

    async fn invoke<P>(
        &self,
        cx: Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Self::Outgoing, Self::Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        (&self).invoke(cx, instance, func, params, paths).await
    }
}

impl<I, O> Invoke for &Oneshot<I, O>
where
    I: AsyncRead + Send + Unpin + 'static,
    O: AsyncWrite + Send + Unpin + 'static,
{
    type Context = ();
    type Outgoing = Outgoing;
    type Incoming = Incoming;

    #[instrument(level = "trace", skip(self, paths, params), fields(params = format!("{params:02x?}")))]
    fn invoke<P>(
        &self,
        (): Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> impl Future<Output = anyhow::Result<(Self::Outgoing, Self::Incoming)>>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        let stream = self.try_take_inner();
        async move {
            let (rx, tx) = stream?;
            invoke(tx, rx, instance, func, params, paths).await
        }
    }
}

impl<I, O> Accept for Oneshot<I, O>
where
    I: AsyncRead + Send + Sync + Unpin + 'static,
    O: AsyncWrite + Send + Sync + Unpin + 'static,
{
    type Context = ();
    type Outgoing = O;
    type Incoming = I;

    async fn accept(&self) -> std::io::Result<(Self::Context, Self::Outgoing, Self::Incoming)> {
        (&self).accept().await
    }
}

impl<I, O> Accept for &Oneshot<I, O>
where
    I: AsyncRead + Send + Sync + Unpin + 'static,
    O: AsyncWrite + Send + Sync + Unpin + 'static,
{
    type Context = ();
    type Outgoing = O;
    type Incoming = I;

    #[instrument(level = "trace", skip(self))]
    async fn accept(&self) -> std::io::Result<(Self::Context, Self::Outgoing, Self::Incoming)> {
        let (rx, tx) = self.try_take_inner()?;
        Ok(((), tx, rx))
    }
}
