//! wRPC transport server handle

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

/// Server serving invocations with [`Send`] futures, such as [`Server`], see [`Server::serve`]
pub trait Serve {
    /// Serves an invocation using `codec` by calling `f` with its parameters, see
    /// [`Server::serve`]
    ///
    /// The future resolves once all I/O of the invocation is done or the peer canceled it.
    fn serve<E, D, T, Fut, Err>(
        self,
        codec: Codec<E, D>,
        f: impl FnOnce(D::Item) -> Fut + Send,
    ) -> impl Future<Output = Result<(), Err>> + Send
    where
        E: Transmit + Encoder<T, Error = std::io::Error> + Send + 'static,
        D: Decoder<Error = std::io::Error> + Receive + Send + 'static,
        D::Item: Send,
        Fut: Future<Output = Result<T, Err>> + Send,
        Err: From<std::io::Error>;
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
    ) -> Result<(), Err>
    where
        E: Transmit + Encoder<T, Error = std::io::Error>,
        D: Decoder<Error = std::io::Error> + Receive,
        Err: From<std::io::Error>,
    {
        let Self {
            tx_rx,
            rx,
            next_tx_tx,
            listener_rx_tx,
        } = self;
        let Codec { mut enc, dec } = codec;
        let (params, mut rx) = receive(dec, rx).await?;
        trace!("received sync parameters");
        future::try_join(
            async {
                (&mut rx).await?;
                _ = listener_rx_tx.send(rx.into_inner());
                Ok::<_, Err>(())
            },
            async {
                let results = f(params).in_current_span().await?;
                let mut data = BytesMut::default();
                enc.encode(results, &mut data)?;
                let mut tx = tx_rx.await.map_err(|_| error_writer_lost())?;
                debug!("transmitting results");
                let fut = transmit_encoded(enc, &mut tx, data)?;
                fut.await?;
                _ = next_tx_tx.send(tx);
                Ok::<_, Err>(())
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
    fn serve<E, D, T, Fut, Err>(
        self,
        codec: Codec<E, D>,
        f: impl FnOnce(D::Item) -> Fut + Send,
    ) -> impl Future<Output = Result<(), Err>> + Send
    where
        E: Transmit + Encoder<T, Error = std::io::Error> + Send + 'static,
        D: Decoder<Error = std::io::Error> + Receive + Send + 'static,
        D::Item: Send,
        Fut: Future<Output = Result<T, Err>> + Send,
        Err: From<std::io::Error>,
    {
        Server::serve(self, codec, async move |params| f(params).await)
    }
}

/// Serves an invocation using `srv` and the default codecs of parameters `P` and results `R`, see
/// [`Server::serve`]
pub fn serve<P, R, S, Fut, Err>(
    srv: S,
    f: impl FnOnce(P) -> Fut + Send,
) -> impl Future<Output = Result<(), Err>> + Send
where
    S: Serve,
    P: Decode<Decoder: Send + 'static> + Send,
    R: Encode<Encoder: Send + 'static>,
    Fut: Future<Output = Result<R, Err>> + Send,
    Err: From<std::io::Error>,
{
    srv.serve(Codec::<R::Encoder, P::Decoder>::default(), f)
}
