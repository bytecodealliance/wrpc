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
    Codec, Decode, Encode, FrameDecoder, FrameEncoder, Receive, Transmit, receive, transmit,
};

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

/// Serves an invocation over the frames written by `tx` and read by `rx` using the default codecs
/// of parameters `P` and results `R`, see [`Codec::serve`]
pub async fn serve<P, R, I, O, Fut, Err>(
    tx: impl BorrowMut<FramedWrite<O, FrameEncoder>>,
    rx: impl BorrowMut<FramedRead<I, FrameDecoder>>,
    f: impl FnOnce(P) -> Fut,
) -> Result<(), Err>
where
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    P: Decode,
    R: Encode,
    Fut: Future<Output = Result<R, Err>>,
    Err: From<std::io::Error>,
{
    Codec::<R::Encoder, P::Decoder>::default()
        .serve(tx, rx, f)
        .await
}
