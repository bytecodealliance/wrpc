//! wRPC transport stream framing

use bytes::Bytes;
use tokio::io::{
    AsyncRead, AsyncWrite, BufReader, DuplexStream, ReadHalf, WriteHalf, duplex, split,
};
use tracing::instrument;

use crate::Invoke;
use crate::frame::{Incoming, IncomingLimits, Outgoing, invoke};

/// [Invoke] implementation in terms of a single stream pair.
///
/// [`Invoke::invoke`] can only be called at most once on [Oneshot],
/// repeated calls will return an error.
#[derive(Debug)]
pub struct Oneshot<I, O> {
    stream: std::sync::Mutex<Option<(I, O)>>,
    limits: IncomingLimits,
}

impl<I, O> From<(I, O)> for Oneshot<I, O> {
    fn from((rx, tx): (I, O)) -> Self {
        Self {
            stream: std::sync::Mutex::new(Some((rx, tx))),
            limits: IncomingLimits::default(),
        }
    }
}

impl From<DuplexStream> for Oneshot<ReadHalf<DuplexStream>, WriteHalf<DuplexStream>> {
    fn from(stream: DuplexStream) -> Self {
        split(stream).into()
    }
}

impl Oneshot<ReadHalf<DuplexStream>, WriteHalf<DuplexStream>> {
    /// Creates a pair of [Oneshot] and server-side [`DuplexStream`] using [`tokio::io::duplex`].
    #[must_use]
    pub fn duplex(max_buf_size: usize) -> (Self, DuplexStream) {
        let (clt, srv) = duplex(max_buf_size);
        (clt.into(), srv)
    }
}

impl<I, O> Oneshot<I, O> {
    /// Bounds the data buffered for the results of the invocation by `limits`, which are the
    /// default [`IncomingLimits`] unless set.
    #[must_use]
    pub fn incoming_limits(self, limits: IncomingLimits) -> Self {
        Self { limits, ..self }
    }

    /// Returns the inner stream pair if [Oneshot] has not been used yet or an error.
    pub fn try_take_inner(&self) -> std::io::Result<(I, O)> {
        match self.stream.try_lock().map(|mut stream| stream.take()) {
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

    async fn invoke<P>(
        &self,
        cx: Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Outgoing, Incoming)>
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

    #[instrument(level = "trace", skip(self, paths, params), fields(params = format!("{params:02x?}")))]
    fn invoke<P>(
        &self,
        (): Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> impl Future<Output = anyhow::Result<(Outgoing, Incoming)>>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        let stream = self.try_take_inner();
        let limits = self.limits;
        async move {
            let (rx, tx) = stream?;
            invoke(
                tx,
                BufReader::new(rx),
                instance,
                func,
                params,
                paths,
                limits,
            )
            .await
        }
    }
}
