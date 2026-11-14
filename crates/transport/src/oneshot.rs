//! [Connect] implementation in terms of a single stream pair

use tokio::io::{AsyncRead, AsyncWrite, DuplexStream, ReadHalf, WriteHalf, duplex, split};
use tracing::instrument;

use crate::Connect;

/// [Connect] implementation in terms of a single stream pair.
///
/// [`Connect::connect`] can only be called at most once on [Oneshot],
/// repeated calls will return an error.
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
    /// Creates a pair of [Oneshot] and server-side [`DuplexStream`] using [`tokio::io::duplex`].
    #[must_use]
    pub fn duplex(max_buf_size: usize) -> (Self, DuplexStream) {
        let (clt, srv) = duplex(max_buf_size);
        (clt.into(), srv)
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

impl<I, O> Connect for Oneshot<I, O>
where
    I: AsyncRead + Send + Unpin + 'static,
    O: AsyncWrite + Send + Unpin + 'static,
{
    type Context = ();
    type Outgoing = O;
    type Incoming = I;
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self))]
    fn connect(
        &self,
        (): Self::Context,
    ) -> impl Future<Output = std::io::Result<(Self::Outgoing, Self::Incoming)>> {
        let stream = self.try_take_inner();
        async move {
            let (rx, tx) = stream?;
            Ok((tx, rx))
        }
    }
}
