//! wRPC transport server handle

use core::borrow::BorrowMut;
use core::error::Error;
use core::fmt;

use futures::future;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Decoder, Encoder, FramedRead, FramedWrite};
use tracing::{Instrument as _, debug, trace};

use crate::{
    Codec, Decode, Encode, FrameDecoder, FrameEncoder, Framed, Receive, ReceiveError, Transmit,
    receive, transmit,
};

/// Error of serving an invocation, by the side at fault
#[derive(Debug)]
pub enum ServeError {
    /// Receiving the parameters or the rest of the invocation failed, such as on malformed,
    /// truncated or unexpected data
    ///
    /// It never holds [`ReceiveError::Unread`], which is [`ServeError::Unread`].
    Request(ReceiveError),
    /// The handler dropped an asynchronous parameter, for which the peer was still sending data
    ///
    /// The handler, not the peer, is at fault.
    Unread,
    /// The handler failed
    Handler(std::io::Error),
    /// Encoding or transmitting the results failed
    Response(std::io::Error),
}

impl fmt::Display for ServeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(..) => f.write_str("failed to receive request"),
            Self::Unread => f.write_str("handler dropped an unread parameter"),
            Self::Handler(..) => f.write_str("handler failed"),
            Self::Response(..) => f.write_str("failed to send response"),
        }
    }
}

impl Error for ServeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Request(err) => Some(err),
            Self::Handler(err) | Self::Response(err) => Some(err),
            Self::Unread => None,
        }
    }
}

impl From<ReceiveError> for ServeError {
    fn from(err: ReceiveError) -> Self {
        match err {
            ReceiveError::Unread => Self::Unread,
            err => Self::Request(err),
        }
    }
}

/// Server serving invocations with [`Send`] futures, such as [`Framed`], see [`Codec::serve`]
pub trait Serve {
    /// Output of serving an invocation, such as the response of a request-response transport
    type Output;

    /// Serves an invocation using `codec` by calling `f` with its parameters, see
    /// [`Codec::serve`]
    ///
    /// The future resolves once all I/O of the invocation is done or the transport took over the
    /// rest of it, such as the response body of a request-response transport.
    fn serve<E, D, T, Fut>(
        self,
        codec: Codec<E, D>,
        f: impl FnOnce(D::Item) -> Fut + Send,
    ) -> impl Future<Output = Result<Self::Output, ServeError>> + Send
    where
        E: Transmit + Encoder<T, Error = std::io::Error> + Send + 'static,
        D: Decoder<Error = std::io::Error> + Receive + Send + 'static,
        D::Item: Send,
        Fut: Future<Output = std::io::Result<T>> + Send;
}

impl<I, O> Serve for Framed<I, O>
where
    I: AsyncRead + Unpin + Send,
    O: AsyncWrite + Unpin + Send,
{
    type Output = ();

    fn serve<E, D, T, Fut>(
        self,
        codec: Codec<E, D>,
        f: impl FnOnce(D::Item) -> Fut + Send,
    ) -> impl Future<Output = Result<(), ServeError>> + Send
    where
        E: Transmit + Encoder<T, Error = std::io::Error> + Send + 'static,
        D: Decoder<Error = std::io::Error> + Receive + Send + 'static,
        D::Item: Send,
        Fut: Future<Output = std::io::Result<T>> + Send,
    {
        let Self { tx, rx } = self;
        codec.serve(tx, rx, async move |params| f(params).await)
    }
}

impl<I, O> Serve for &mut Framed<I, O>
where
    I: AsyncRead + Unpin + Send,
    O: AsyncWrite + Unpin + Send,
{
    type Output = ();

    fn serve<E, D, T, Fut>(
        self,
        codec: Codec<E, D>,
        f: impl FnOnce(D::Item) -> Fut + Send,
    ) -> impl Future<Output = Result<(), ServeError>> + Send
    where
        E: Transmit + Encoder<T, Error = std::io::Error> + Send + 'static,
        D: Decoder<Error = std::io::Error> + Receive + Send + 'static,
        D::Item: Send,
        Fut: Future<Output = std::io::Result<T>> + Send,
    {
        let Framed { tx, rx } = self;
        codec.serve(tx, rx, async move |params| f(params).await)
    }
}

impl<E, D> Codec<E, D>
where
    E: Transmit,
    D: Decoder<Error = std::io::Error> + Receive,
{
    /// Serves an invocation over the frames written by `tx` and read by `rx`, which follow the
    /// header read, for example, by [`Listener::accept`](crate::Listener::accept)
    ///
    /// Receives the parameters, calls `f` with them and transmits the results it returns. The
    /// asynchronous parameters are received while `f` runs and the results are transmitted, it
    /// resolves once all I/O is done. Nothing is transmitted if `f` fails.
    ///
    /// Unlike [`Serve`], this does not require [`Send`] values and futures.
    pub async fn serve<F, W, I, O, T>(
        self,
        tx: W,
        rx: F,
        f: impl AsyncFnOnce(D::Item) -> std::io::Result<T>,
    ) -> Result<(), ServeError>
    where
        F: BorrowMut<FramedRead<I, FrameDecoder>>,
        W: BorrowMut<FramedWrite<O, FrameEncoder>>,
        I: AsyncRead + Unpin,
        O: AsyncWrite + Unpin,
        E: Encoder<T, Error = std::io::Error>,
    {
        let Self { enc, dec } = self;
        let (params, rx) = receive(dec, rx).await?;
        trace!("received sync parameters");
        future::try_join(async { rx.await.map_err(ServeError::from) }, async {
            let results = f(params)
                .in_current_span()
                .await
                .map_err(ServeError::Handler)?;
            debug!("transmitting results");
            let fut = transmit(enc, tx, results).map_err(ServeError::Response)?;
            fut.await.map_err(ServeError::Response)
        })
        .await?;
        Ok(())
    }
}

/// Serves an invocation using `srv` and the default codecs of parameters `P` and results `R`, see
/// [`Codec::serve`]
pub fn serve<P, R, S, Fut>(
    srv: S,
    f: impl FnOnce(P) -> Fut + Send,
) -> impl Future<Output = Result<S::Output, ServeError>> + Send
where
    S: Serve,
    P: Decode<Decoder: Send + 'static> + Send,
    R: Encode<Encoder: Send + 'static>,
    Fut: Future<Output = std::io::Result<R>> + Send,
{
    srv.serve(Codec::<R::Encoder, P::Decoder>::default(), f)
}
