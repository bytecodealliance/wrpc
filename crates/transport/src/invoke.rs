//! wRPC transport client handle

use core::borrow::BorrowMut;
use core::future::poll_fn;
use core::pin::{Pin, pin};
use core::task::{Context, Poll, ready};

use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Decoder, Encoder, FramedRead, FramedWrite};
use tracing::{Instrument as _, debug, instrument, trace};

use crate::{
    Codec, Decode, Encode, FrameDecoder, FrameEncoder, HeaderEncoder, HeaderRef, Receipt, Receive,
    Transmission, Transmit, receive, transmit,
};

impl<E, D> Codec<E, D>
where
    E: Transmit,
    D: Decoder<Error = std::io::Error> + Receive,
{
    /// Invokes function `func` on instance `instance` over the frames written by `tx` and read by
    /// `rx`
    ///
    /// `params` are encoded before this returns, the invocation header is sent along with the
    /// first frame. Returns the synchronous results along with the [Invocation], which completes
    /// once the asynchronous parameters are transmitted and the asynchronous results are received,
    /// if there are any. Nothing is transmitted or received unless it is polled.
    ///
    /// `tx` is never shut down, but dropped as soon as the parameters are transmitted, which ends
    /// the request of request-response transports.
    ///
    /// To reuse a connection, pass `tx` and `rx` by `&mut` and start the next invocation once the
    /// [Invocation] succeeded; once one failed, drop them. To pipeline invocations, write the
    /// header of each and [`transmit`] its parameters, then [`receive`] the results in order.
    #[instrument(level = "trace", skip(self, tx, rx, params))]
    pub fn invoke<F, W, I, O, T>(
        self,
        mut tx: W,
        rx: F,
        instance: &str,
        func: &str,
        params: T,
    ) -> impl Future<
        Output = std::io::Result<(D::Item, Invocation<Transmission<W, O, E>, Receipt<F, I, D>>)>,
    > + use<F, W, I, O, E, T, D>
    where
        F: BorrowMut<FramedRead<I, FrameDecoder>>,
        W: BorrowMut<FramedWrite<O, FrameEncoder>>,
        I: AsyncRead + Unpin,
        O: AsyncWrite + Unpin,
        E: Encoder<T, Error = std::io::Error>,
    {
        let Self { enc, dec } = self;
        let res = HeaderEncoder
            .encode(
                HeaderRef {
                    instance,
                    name: func,
                },
                tx.borrow_mut().write_buffer_mut(),
            )
            .and_then(|()| transmit(enc, tx, params));
        async move {
            let tx = res?;
            debug!("invoking function");
            let mut tx = Some(tx);
            let (results, rx) = {
                let mut results = pin!(receive(dec, rx));
                poll_fn(|cx| {
                    if let Poll::Ready(results) = results.as_mut().poll(cx) {
                        return Poll::Ready(results);
                    }
                    if let Some(fut) = &mut tx {
                        ready!(Pin::new(fut).poll(cx))?;
                        tx = None;
                    }
                    Poll::Pending
                })
                .await?
            };
            trace!("received sync results");
            Ok((results, Invocation { tx, rx: Some(rx) }))
        }
        .in_current_span()
    }

    /// Invokes function `func` on instance `instance`, see [`Codec::invoke`]
    ///
    /// This is like [`Codec::invoke`], but it only resolves once all I/O is done
    pub async fn invoke_blocking<I, O, T>(
        self,
        tx: impl BorrowMut<FramedWrite<O, FrameEncoder>>,
        rx: impl BorrowMut<FramedRead<I, FrameDecoder>>,
        instance: &str,
        func: &str,
        params: T,
    ) -> std::io::Result<D::Item>
    where
        I: AsyncRead + Unpin,
        O: AsyncWrite + Unpin,
        E: Encoder<T, Error = std::io::Error>,
    {
        let (ret, io) = self.invoke(tx, rx, instance, func, params).await?;
        trace!("awaiting I/O completion");
        io.await?;
        Ok(ret)
    }
}

/// Invokes function `func` on instance `instance` over the frames written by `tx` and read by `rx`
/// using the default codecs of `params` and results `R`, see [`Codec::invoke`]
pub fn invoke<P, R, F, W, I, O>(
    tx: W,
    rx: F,
    instance: &str,
    func: &str,
    params: P,
) -> impl Future<
    Output = std::io::Result<(
        R,
        Invocation<Transmission<W, O, P::Encoder>, Receipt<F, I, R::Decoder>>,
    )>,
> + use<P, R, F, W, I, O>
where
    F: BorrowMut<FramedRead<I, FrameDecoder>>,
    W: BorrowMut<FramedWrite<O, FrameEncoder>>,
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    P: Encode,
    R: Decode,
{
    Codec::<P::Encoder, R::Decoder>::default().invoke(tx, rx, instance, func, params)
}

/// Future transmitting the parameters and receiving the results of an invocation, see
/// [`Codec::invoke`]
#[must_use]
pub struct Invocation<T, U> {
    tx: Option<T>,
    rx: Option<U>,
}

impl<T, U> Future for Invocation<T, U>
where
    T: Future<Output = std::io::Result<()>> + Unpin,
    U: Future<Output = std::io::Result<()>> + Unpin,
{
    type Output = std::io::Result<()>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(tx) = &mut self.tx
            && let Poll::Ready(res) = Pin::new(tx).poll(cx)
        {
            res?;
            self.tx = None;
        }
        if let Some(rx) = &mut self.rx
            && let Poll::Ready(res) = Pin::new(rx).poll(cx)
        {
            res?;
            self.rx = None;
        }
        if self.tx.is_some() || self.rx.is_some() {
            return Poll::Pending;
        }
        Poll::Ready(Ok(()))
    }
}

/// Invokes function `func` on instance `instance` using the default codecs of `params` and
/// results `R`, see [`Codec::invoke_blocking`]
pub async fn invoke_blocking<P, R, I, O>(
    tx: impl BorrowMut<FramedWrite<O, FrameEncoder>>,
    rx: impl BorrowMut<FramedRead<I, FrameDecoder>>,
    instance: &str,
    func: &str,
    params: P,
) -> std::io::Result<R>
where
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    P: Encode,
    R: Decode,
{
    Codec::<P::Encoder, R::Decoder>::default()
        .invoke_blocking(tx, rx, instance, func, params)
        .await
}
