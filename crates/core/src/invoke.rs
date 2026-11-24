//! wRPC transport client handle

use core::borrow::BorrowMut;
use core::error::Error;
use core::fmt;
use core::future::poll_fn;
use core::pin::{Pin, pin};
use core::task::{Context, Poll, ready};

use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Decoder, Encoder, FramedRead, FramedWrite};
use tracing::{Instrument as _, debug, instrument, trace};

use crate::{
    Codec, Decode, Encode, FrameDecoder, FrameEncoder, Framed, HeaderEncoder, HeaderRef, Receipt,
    Receive, ReceiveError, Transmission, Transmit, receive, transmit,
};

/// Error of invoking a function, by the side at fault
#[derive(Debug)]
pub enum InvokeError {
    /// The request failed, such as on encoding or transmitting the parameters, or on the
    /// transport rejecting it before any response
    Request(std::io::Error),
    /// The caller dropped an asynchronous result, for which the peer was still sending data
    ///
    /// The caller, not the peer, is at fault.
    Unread,
    /// Receiving the results failed, such as on malformed, truncated or unexpected data
    ///
    /// It never holds [`ReceiveError::Unread`], which is [`InvokeError::Unread`].
    Response(ReceiveError),
}

impl fmt::Display for InvokeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(..) => f.write_str("request failed"),
            Self::Unread => f.write_str("caller dropped an unread result"),
            Self::Response(..) => f.write_str("failed to receive response"),
        }
    }
}

impl Error for InvokeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Request(err) => Some(err),
            Self::Unread => None,
            Self::Response(err) => Some(err),
        }
    }
}

impl From<ReceiveError> for InvokeError {
    fn from(err: ReceiveError) -> Self {
        match err {
            ReceiveError::Unread => Self::Unread,
            err => Self::Response(err),
        }
    }
}

impl From<InvokeError> for std::io::Error {
    fn from(err: InvokeError) -> Self {
        match err {
            InvokeError::Request(err) => err,
            InvokeError::Unread => Self::new(std::io::ErrorKind::BrokenPipe, err),
            InvokeError::Response(err) => err.into(),
        }
    }
}

/// Client invoking functions with parameters encoded by `E`, such as [`Framed`], see
/// [`Codec::invoke`]
///
/// The asynchronous results must be consumed concurrently with each other and with the
/// [`Invoke::Invocation`], see the [crate docs](crate).
pub trait Invoke<E: Transmit> {
    /// Future transmitting the asynchronous parameters and receiving the asynchronous results of
    /// an invocation
    type Invocation<D: Receive>: Future<Output = Result<(), InvokeError>>;

    /// Invokes function `func` on instance `instance` with `params` using `codec`, see
    /// [`Codec::invoke`]
    fn invoke<D, T>(
        self,
        codec: Codec<E, D>,
        instance: &str,
        func: &str,
        params: T,
    ) -> impl Future<Output = Result<(D::Item, Self::Invocation<D>), InvokeError>> + use<Self, E, D, T>
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
        Transmission<E, FramedWrite<O, FrameEncoder>, O>,
        Receipt<D, FramedRead<I, FrameDecoder>, I>,
    >;

    fn invoke<D, T>(
        self,
        codec: Codec<E, D>,
        instance: &str,
        func: &str,
        params: T,
    ) -> impl Future<Output = Result<(D::Item, Self::Invocation<D>), InvokeError>> + use<I, O, E, D, T>
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
        Transmission<E, &'a mut FramedWrite<O, FrameEncoder>, O>,
        Receipt<D, &'a mut FramedRead<I, FrameDecoder>, I>,
    >;

    fn invoke<D, T>(
        self,
        codec: Codec<E, D>,
        instance: &str,
        func: &str,
        params: T,
    ) -> impl Future<Output = Result<(D::Item, Self::Invocation<D>), InvokeError>> + use<'a, I, O, E, D, T>
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
    /// if there are any. Nothing is transmitted or received unless it is polled. The asynchronous
    /// results must be consumed concurrently with each other and with the [Invocation], see the
    /// [crate docs](crate).
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
        Output = Result<
            (D::Item, Invocation<Transmission<E, W, O>, Receipt<D, F, I>>),
            InvokeError,
        >,
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
            let tx = res.map_err(InvokeError::Request)?;
            debug!("invoking function");
            let mut tx = Some(tx);
            let (results, rx) = {
                let mut results = pin!(receive(dec, rx));
                poll_fn(|cx| {
                    if let Poll::Ready(results) = results.as_mut().poll(cx) {
                        return Poll::Ready(results.map_err(InvokeError::from));
                    }
                    if let Some(fut) = &mut tx {
                        ready!(Pin::new(fut).poll(cx)).map_err(InvokeError::Request)?;
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
}

/// Invokes function `func` on instance `instance` using `clt` and the default codecs of `params`
/// and results `R`, see [`Codec::invoke`]
pub fn invoke<P, R, C>(
    clt: C,
    instance: &str,
    func: &str,
    params: P,
) -> impl Future<Output = Result<(R, C::Invocation<R::Decoder>), InvokeError>> + use<P, R, C>
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
    U: Future<Output = Result<(), ReceiveError>> + Unpin,
{
    type Output = Result<(), InvokeError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(tx) = &mut self.tx
            && let Poll::Ready(res) = Pin::new(tx).poll(cx)
        {
            res.map_err(InvokeError::Request)?;
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
