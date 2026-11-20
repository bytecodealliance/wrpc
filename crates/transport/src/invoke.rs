//! wRPC transport client handle

use core::borrow::BorrowMut;
use core::future::poll_fn;
use core::pin::{Pin, pin};
use core::task::{Context, Poll, ready};

use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Decoder, Encoder, FramedRead, FramedWrite};
use tracing::{Instrument as _, debug, instrument, trace};

use crate::{
    Codec, Decode, Encode, FrameDecoder, FrameEncoder, Framed, HeaderEncoder, HeaderRef, Receipt,
    Receive, Transmission, Transmit, receive, transmit,
};

/// Client invoking functions with parameters encoded by `E`, such as [`Framed`], see
/// [`Codec::invoke`]
pub trait Invoke<E: Transmit> {
    /// Future transmitting the asynchronous parameters and receiving the asynchronous results of
    /// an invocation
    type Invocation<D: Receive>: Future<Output = std::io::Result<()>>;

    /// Invokes function `func` on instance `instance` with `params` using `codec`, see
    /// [`Codec::invoke`]
    fn invoke<D, T>(
        self,
        codec: Codec<E, D>,
        instance: &str,
        func: &str,
        params: T,
    ) -> impl Future<Output = std::io::Result<(D::Item, Self::Invocation<D>)>> + use<Self, E, D, T>
    where
        E: Encoder<T, Error = std::io::Error>,
        D: Decoder<Error = std::io::Error> + Receive;
}

impl<I, O, E> Invoke<E> for Framed<I, O>
where
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    E: Transmit,
{
    type Invocation<D: Receive> = Invocation<
        Transmission<FramedWrite<O, FrameEncoder>, O, E>,
        Receipt<FramedRead<I, FrameDecoder>, I, D>,
    >;

    fn invoke<D, T>(
        self,
        codec: Codec<E, D>,
        instance: &str,
        func: &str,
        params: T,
    ) -> impl Future<Output = std::io::Result<(D::Item, Self::Invocation<D>)>> + use<I, O, E, D, T>
    where
        E: Encoder<T, Error = std::io::Error>,
        D: Decoder<Error = std::io::Error> + Receive,
    {
        let Self { tx, rx } = self;
        codec.invoke(tx, rx, instance, func, params)
    }
}

impl<'a, I, O, E> Invoke<E> for &'a mut Framed<I, O>
where
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    E: Transmit,
{
    type Invocation<D: Receive> = Invocation<
        Transmission<&'a mut FramedWrite<O, FrameEncoder>, O, E>,
        Receipt<&'a mut FramedRead<I, FrameDecoder>, I, D>,
    >;

    fn invoke<D, T>(
        self,
        codec: Codec<E, D>,
        instance: &str,
        func: &str,
        params: T,
    ) -> impl Future<Output = std::io::Result<(D::Item, Self::Invocation<D>)>> + use<'a, I, O, E, D, T>
    where
        E: Encoder<T, Error = std::io::Error>,
        D: Decoder<Error = std::io::Error> + Receive,
    {
        let Framed { tx, rx } = self;
        codec.invoke(tx, rx, instance, func, params)
    }
}

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

/// Invokes function `func` on instance `instance` using `clt` and the default codecs of `params`
/// and results `R`, see [`Codec::invoke`]
pub fn invoke<P, R, C>(
    clt: C,
    instance: &str,
    func: &str,
    params: P,
) -> impl Future<Output = std::io::Result<(R, C::Invocation<R::Decoder>)>> + use<P, R, C>
where
    C: Invoke<P::Encoder>,
    P: Encode,
    R: Decode,
{
    clt.invoke(
        Codec::<P::Encoder, R::Decoder>::default(),
        instance,
        func,
        params,
    )
}

/// Invokes function `func` on instance `instance` using `clt` and the default codecs of `params`
/// and results `R`, see [`Codec::invoke_blocking`]
pub async fn invoke_blocking<P, R, C>(
    clt: C,
    instance: &str,
    func: &str,
    params: P,
) -> std::io::Result<R>
where
    C: Invoke<P::Encoder>,
    P: Encode,
    R: Decode,
{
    let (ret, io) = invoke(clt, instance, func, params).await?;
    trace!("awaiting I/O completion");
    io.await?;
    Ok(ret)
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
