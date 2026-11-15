//! wRPC transport client handle

use core::borrow::BorrowMut;
use core::future::poll_fn;
use core::pin::{Pin, pin};
use core::task::{Context, Poll, ready};

use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Decoder, Encoder, FramedRead, FramedWrite};
use tracing::{Instrument as _, debug, instrument, trace};

use crate::frame::{self, Outgoing, Receipt, Transmission};
use crate::mux::ingress;
use crate::{Decode, Encode, HeaderEncoder, HeaderRef, Receive, Transmit};

/// Invoker of functions, which encodes parameters using `enc` and decodes results using `dec`
#[derive(Clone, Copy, Debug, Default)]
pub struct Invoker<E, D> {
    /// Parameter encoder
    pub enc: E,
    /// Result decoder
    pub dec: D,
}

impl<E, D> Invoker<E, D>
where
    E: Transmit,
    D: Decoder<Error = std::io::Error> + Receive,
{
    /// Sends the invocation of function `func` on instance `instance` with `params` on the byte
    /// stream `tx`
    ///
    /// `params` are encoded before this returns. Returns the [Transmission] of the parameters and
    /// the [Pending] results, which are to be received from the byte stream, which `tx` is the
    /// sending half of.
    ///
    /// To pipeline invocations on one byte stream, pass `tx` by `&mut`, send the next invocation
    /// once the [Transmission] completed and receive the results in order using one
    /// [`FramedRead`]. Keep receiving results while sending, since the peer may not read the next
    /// invocation before the results of the previous one are received.
    pub fn send<O, T>(
        self,
        tx: O,
        instance: &str,
        func: &str,
        params: T,
    ) -> std::io::Result<(Transmission<O, E>, Pending<D>)>
    where
        O: AsyncWrite + Unpin,
        E: Encoder<T, Error = std::io::Error>,
    {
        let Self { enc, dec } = self;
        let mut tx = FramedWrite::new(tx, frame::Encoder);
        HeaderEncoder.encode(
            HeaderRef {
                instance,
                name: func,
            },
            tx.write_buffer_mut(),
        )?;
        let tx = Outgoing { tx, enc }.send(params)?;
        Ok((tx, Pending { dec }))
    }

    /// Invokes function `func` on instance `instance` over the byte streams `tx` and `rx`
    ///
    /// `params` are encoded before this returns, the invocation header is sent along with the
    /// first frame. Returns the synchronous results along with the [Invocation], which completes
    /// once the asynchronous parameters are transmitted and the asynchronous results are received,
    /// if there are any. Nothing is transmitted or received unless it is polled.
    ///
    /// `tx` is never shut down, but dropped as soon as the parameters are transmitted, which ends
    /// the request of request-response transports.
    ///
    /// To reuse the byte streams, pass `tx` and `rx` by `&mut` and start the next invocation once
    /// the [Invocation] succeeded.
    #[instrument(level = "trace", skip(self, tx, rx, params))]
    pub fn invoke<I, O, T>(
        self,
        tx: O,
        rx: I,
        instance: &str,
        func: &str,
        params: T,
    ) -> impl Future<Output = std::io::Result<(D::Item, Invocation<I, O, E, D>)>> + use<I, O, E, T, D>
    where
        I: AsyncRead + Unpin,
        O: AsyncWrite + Unpin,
        E: Encoder<T, Error = std::io::Error>,
    {
        let res = self.send(tx, instance, func, params);
        async move {
            let (tx, pending) = res?;
            debug!("invoking function");
            let mut tx = Some(tx);
            let (results, rx) = {
                let rx = FramedRead::new(rx, frame::Decoder::default());
                let mut results = pin!(pending.receive(rx));
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

    /// Invokes function `func` on instance `instance`, see [`Invoker::invoke`]
    ///
    /// This is like [`Invoker::invoke`], but it only resolves once all I/O is done
    pub async fn invoke_blocking<I, O, T>(
        self,
        tx: O,
        rx: I,
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

/// Results of an invocation to be received, see [`Invoker::send`]
#[derive(Debug)]
#[must_use]
pub struct Pending<T> {
    dec: T,
}

impl<T> Pending<T>
where
    T: Decoder<Error = std::io::Error> + Receive,
{
    /// Receives the results from the frames `rx`
    ///
    /// Returns the synchronous results and the [Receipt] of the asynchronous results. No frame
    /// past the results is read from `rx`, so pass it by `&mut` to receive the results of the next
    /// invocation from it.
    pub fn receive<F, I>(
        self,
        rx: F,
    ) -> impl Future<Output = std::io::Result<(T::Item, Receipt<F, I, T>)>>
    where
        F: BorrowMut<FramedRead<I, frame::Decoder>>,
        I: AsyncRead + Unpin,
    {
        ingress(rx, self.dec)
    }
}

/// Invokes function `func` on instance `instance` over the byte streams `tx` and `rx` using the
/// default codecs of `params` and results `R`, see [`Invoker::invoke`]
pub fn invoke<P, R, I, O>(
    tx: O,
    rx: I,
    instance: &str,
    func: &str,
    params: P,
) -> impl Future<Output = std::io::Result<(R, Invocation<I, O, P::Encoder, R::Decoder>)>> + use<P, R, I, O>
where
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    P: Encode,
    R: Decode,
{
    Invoker::<P::Encoder, R::Decoder>::default().invoke(tx, rx, instance, func, params)
}

/// Future transmitting the parameters and receiving the results of an invocation, see
/// [`Invoker::invoke`]
#[must_use]
pub struct Invocation<I, O, E, D> {
    tx: Option<Transmission<O, E>>,
    rx: Option<Receipt<FramedRead<I, frame::Decoder>, I, D>>,
}

impl<I, O, E, D> Future for Invocation<I, O, E, D>
where
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    E: Transmit,
    D: Receive,
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
/// results `R`, see [`Invoker::invoke_blocking`]
pub async fn invoke_blocking<P, R>(
    tx: impl AsyncWrite + Unpin,
    rx: impl AsyncRead + Unpin,
    instance: &str,
    func: &str,
    params: P,
) -> std::io::Result<R>
where
    P: Encode,
    R: Decode,
{
    Invoker::<P::Encoder, R::Decoder>::default()
        .invoke_blocking(tx, rx, instance, func, params)
        .await
}
