//! wRPC transport server handle

use core::borrow::BorrowMut;
use core::future::poll_fn;
use core::pin::{Pin, pin};
use core::task::Poll;

use futures::future;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Decoder, Encoder, FramedRead, FramedWrite};
use tracing::{Instrument as _, debug, trace};

use crate::{
    Codec, Decode, Encode, FrameDecoder, FrameEncoder, Framed, Receive, Transmit, receive, transmit,
};

/// Server serving invocations with [`Send`] futures, such as [`Framed`], see [`Codec::serve`]
pub trait Serve {
    /// Value an invocation is served with
    type Output;

    /// Serves an invocation using `codec` by calling `f` with its parameters, see
    /// [`Codec::serve`]
    fn serve<E, D, T, Fut, Err>(
        self,
        codec: Codec<E, D>,
        f: impl FnOnce(D::Item) -> Fut + Send,
    ) -> impl Future<Output = Result<Self::Output, Err>> + Send
    where
        E: Transmit + Encoder<T, Error = std::io::Error> + Send + 'static,
        D: Decoder<Error = std::io::Error> + Receive + Send + 'static,
        D::Item: Send,
        Fut: Future<Output = Result<T, Err>> + Send,
        Err: From<std::io::Error>;
}

impl<I, O> Serve for Framed<I, O>
where
    I: AsyncRead + Unpin + Send,
    O: AsyncWrite + Unpin + Send,
{
    type Output = ();

    fn serve<E, D, T, Fut, Err>(
        self,
        codec: Codec<E, D>,
        f: impl FnOnce(D::Item) -> Fut + Send,
    ) -> impl Future<Output = Result<Self::Output, Err>> + Send
    where
        E: Transmit + Encoder<T, Error = std::io::Error> + Send + 'static,
        D: Decoder<Error = std::io::Error> + Receive + Send + 'static,
        D::Item: Send,
        Fut: Future<Output = Result<T, Err>> + Send,
        Err: From<std::io::Error>,
    {
        let Self { tx, rx } = self;
        codec.serve(tx, rx, f)
    }
}

impl<I, O> Serve for &mut Framed<I, O>
where
    I: AsyncRead + Unpin + Send,
    O: AsyncWrite + Unpin + Send,
{
    type Output = ();

    fn serve<E, D, T, Fut, Err>(
        self,
        codec: Codec<E, D>,
        f: impl FnOnce(D::Item) -> Fut + Send,
    ) -> impl Future<Output = Result<Self::Output, Err>> + Send
    where
        E: Transmit + Encoder<T, Error = std::io::Error> + Send + 'static,
        D: Decoder<Error = std::io::Error> + Receive + Send + 'static,
        D::Item: Send,
        Fut: Future<Output = Result<T, Err>> + Send,
        Err: From<std::io::Error>,
    {
        let Framed { tx, rx } = self;
        codec.serve(tx, rx, f)
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
    pub async fn serve<I, O, T, Fut, Err>(
        self,
        tx: impl BorrowMut<FramedWrite<O, FrameEncoder>>,
        rx: impl BorrowMut<FramedRead<I, FrameDecoder>>,
        f: impl FnOnce(D::Item) -> Fut,
    ) -> Result<(), Err>
    where
        I: AsyncRead + Unpin,
        O: AsyncWrite + Unpin,
        E: Encoder<T, Error = std::io::Error>,
        Fut: Future<Output = Result<T, Err>>,
        Err: From<std::io::Error>,
    {
        let Self { enc, dec } = self;
        let (params, rx) = receive(dec, rx).await?;
        trace!("received sync parameters");
        let mut rx = Some(rx);
        let results = {
            let mut results = pin!(f(params).in_current_span());
            poll_fn(|cx| {
                if let Some(fut) = &mut rx
                    && let Poll::Ready(res) = Pin::new(fut).poll(cx)
                {
                    res?;
                    rx = None;
                }
                results.as_mut().poll(cx)
            })
            .await?
        };
        debug!("transmitting results");
        let tx = transmit(enc, tx, results)?;
        if let Some(rx) = rx {
            future::try_join(tx, rx).await?;
        } else {
            tx.await?;
        }
        Ok(())
    }
}

/// Serves an invocation using `srv` and the default codecs of parameters `P` and results `R`, see
/// [`Codec::serve`]
pub fn serve<P, R, S, Fut, Err>(
    srv: S,
    f: impl FnOnce(P) -> Fut + Send,
) -> impl Future<Output = Result<S::Output, Err>> + Send
where
    S: Serve,
    P: Decode<Decoder: Send + 'static> + Send,
    R: Encode<Encoder: Send + 'static>,
    Fut: Future<Output = Result<R, Err>> + Send,
    Err: From<std::io::Error>,
{
    srv.serve(Codec::<R::Encoder, P::Decoder>::default(), f)
}
