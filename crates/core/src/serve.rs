//! wRPC transport server handle

use core::error::Error;
use core::fmt;

use bytes::BytesMut;
use futures::future;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::oneshot;
use tokio_util::codec::{Decoder, Encoder, FramedRead, FramedWrite};
use tracing::{Instrument as _, debug, trace};

use crate::mux::transmit_encoded;
use crate::{
    Codec, Decode, Encode, FrameDecoder, FrameEncoder, Receive, Transmit, error_writer_lost,
    receive,
};

/// Error of serving an invocation, by the side at fault
#[derive(Debug)]
pub enum ServeError<E> {
    /// Receiving the parameters or the rest of the invocation failed, such as on malformed,
    /// truncated or unexpected data, or on data for an asynchronous parameter the handler dropped
    Request(std::io::Error),
    /// The handler failed
    Handler(E),
    /// Encoding or transmitting the results failed
    Response(std::io::Error),
}

impl<E: fmt::Display> fmt::Display for ServeError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(..) => f.write_str("failed to receive request"),
            Self::Handler(err) => write!(f, "{err:#}"),
            Self::Response(..) => f.write_str("failed to send response"),
        }
    }
}

impl<E: fmt::Debug + fmt::Display> Error for ServeError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Request(err) | Self::Response(err) => Some(err),
            Self::Handler(..) => None,
        }
    }
}

/// Server serving invocations with [`Send`] futures, such as [`Server`], see [`Server::serve`]
pub trait Serve {
    /// Output of serving an invocation, such as the response of a request-response transport
    type Output;

    /// Serves an invocation using `codec` by calling `f` with its parameters, see
    /// [`Server::serve`]
    ///
    /// The future resolves once all I/O of the invocation is done or the transport took over the
    /// rest of it, such as the response body of a request-response transport.
    fn serve<E, D, T, Fut, Err>(
        self,
        codec: Codec<E, D>,
        f: impl FnOnce(D::Item) -> Fut + Send,
    ) -> impl Future<Output = Result<Self::Output, ServeError<Err>>> + Send
    where
        E: Transmit + Encoder<T, Error = std::io::Error> + Send + 'static,
        D: Decoder<Error = std::io::Error> + Receive + Send + 'static,
        D::Item: Send,
        Fut: Future<Output = Result<T, Err>> + Send;
}

/// Server serving an invocation accepted by a [`Listener`](crate::Listener)
#[derive(Debug)]
pub struct Server<I, O> {
    pub(crate) tx_rx: oneshot::Receiver<FramedWrite<O, FrameEncoder>>,
    pub(crate) rx: FramedRead<I, FrameDecoder>,
    pub(crate) next_tx_tx: oneshot::Sender<FramedWrite<O, FrameEncoder>>,
    pub(crate) listener_rx_tx: oneshot::Sender<FramedRead<I, FrameDecoder>>,
}

impl<I, O> Server<I, O>
where
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
{
    /// Serves the invocation using `codec` by calling `f` with its parameters
    ///
    /// Receives the parameters, calls `f` with them and transmits the results it returns, once
    /// the results of the previously accepted invocations are transmitted. The asynchronous
    /// parameters are received while `f` runs and the results are transmitted, the next
    /// invocation is accepted once they are received. Resolves once all I/O is done. Nothing is
    /// transmitted if `f` fails.
    ///
    /// Unlike [`Serve`], this does not require [`Send`] values and futures.
    pub async fn serve<E, D, T, Err>(
        self,
        codec: Codec<E, D>,
        f: impl AsyncFnOnce(D::Item) -> Result<T, Err>,
    ) -> Result<(), ServeError<Err>>
    where
        E: Transmit + Encoder<T, Error = std::io::Error>,
        D: Decoder<Error = std::io::Error> + Receive,
    {
        let Self {
            tx_rx,
            rx,
            next_tx_tx,
            listener_rx_tx,
        } = self;
        let Codec { mut enc, dec } = codec;
        let (params, mut rx) = receive(dec, rx).await.map_err(ServeError::Request)?;
        trace!("received sync parameters");
        future::try_join(
            async {
                (&mut rx).await.map_err(ServeError::Request)?;
                _ = listener_rx_tx.send(rx.into_inner());
                Ok(())
            },
            async {
                let results = f(params)
                    .in_current_span()
                    .await
                    .map_err(ServeError::Handler)?;
                let mut data = BytesMut::default();
                enc.encode(results, &mut data)
                    .map_err(ServeError::Response)?;
                let mut tx = tx_rx
                    .await
                    .map_err(|_| ServeError::Response(error_writer_lost()))?;
                debug!("transmitting results");
                let fut = transmit_encoded(enc, &mut tx, data).map_err(ServeError::Response)?;
                fut.await.map_err(ServeError::Response)?;
                _ = next_tx_tx.send(tx);
                Ok(())
            },
        )
        .await?;
        Ok(())
    }
}

impl<I, O> Serve for Server<I, O>
where
    I: AsyncRead + Unpin + Send,
    O: AsyncWrite + Unpin + Send,
{
    type Output = ();

    fn serve<E, D, T, Fut, Err>(
        self,
        codec: Codec<E, D>,
        f: impl FnOnce(D::Item) -> Fut + Send,
    ) -> impl Future<Output = Result<(), ServeError<Err>>> + Send
    where
        E: Transmit + Encoder<T, Error = std::io::Error> + Send + 'static,
        D: Decoder<Error = std::io::Error> + Receive + Send + 'static,
        D::Item: Send,
        Fut: Future<Output = Result<T, Err>> + Send,
    {
        Server::serve(self, codec, async move |params| f(params).await)
    }
}

/// Serves an invocation using `srv` and the default codecs of parameters `P` and results `R`, see
/// [`Server::serve`]
pub fn serve<P, R, S, Fut, Err>(
    srv: S,
    f: impl FnOnce(P) -> Fut + Send,
) -> impl Future<Output = Result<S::Output, ServeError<Err>>> + Send
where
    S: Serve,
    P: Decode<Decoder: Send + 'static> + Send,
    R: Encode<Encoder: Send + 'static>,
    Fut: Future<Output = Result<R, Err>> + Send,
{
    srv.serve(Codec::<R::Encoder, P::Decoder>::default(), f)
}
