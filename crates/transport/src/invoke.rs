//! wRPC transport client handle

use core::future::poll_fn;
use core::pin::{Pin, pin};
use core::task::{Context, Poll, ready};

use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Decoder, Encoder, FramedRead, FramedWrite};
use tracing::{Instrument as _, debug, instrument, trace};

use crate::frame::{self, Outgoing, Transmission};
use crate::mux::{Receipt, ingress};
use crate::{HeaderEncoder, HeaderRef, Receive, Transmit};

/// Invokes function `func` on instance `instance` over the byte streams `tx` and `rx`, encoding
/// `params` using `enc` and decoding the results using `dec`
///
/// `params` are encoded before this returns, the invocation header is sent along with the first
/// frame. Returns the synchronous results along with the [Invocation], which completes once the
/// asynchronous parameters are transmitted and the asynchronous results are received, if there
/// are any. Nothing is transmitted or received unless it is polled.
///
/// `tx` is never shut down, but dropped as soon as the parameters are transmitted, which ends the
/// request of request-response transports.
///
/// To reuse the byte streams, pass `tx` and `rx` by `&mut` and start the next invocation once the
/// [Invocation] succeeded.
#[instrument(level = "trace", skip(tx, rx, enc, params, dec))]
pub fn invoke<I, O, E, T, D>(
    tx: O,
    rx: I,
    instance: &str,
    func: &str,
    enc: E,
    params: T,
    dec: D,
) -> impl Future<
    Output = std::io::Result<(
        D::Item,
        Invocation<Transmission<O, E>, Receipt<FramedRead<I, frame::Decoder>, I, D>>,
    )>,
> + use<I, O, E, T, D>
where
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    E: Encoder<T, Error = std::io::Error> + Transmit,
    D: Decoder<Error = std::io::Error> + Receive,
{
    let mut tx = FramedWrite::new(tx, frame::Encoder);
    let header = HeaderEncoder.encode(
        HeaderRef {
            instance,
            name: func,
        },
        tx.write_buffer_mut(),
    );
    let tx = header.and_then(|()| Outgoing { tx }.send(enc, params));
    async move {
        let tx = tx?;
        debug!("invoking function");
        let mut tx = Some(tx);
        let (results, rx) = {
            let rx = FramedRead::new(rx, frame::Decoder::default());
            let mut results = pin!(ingress(rx, dec));
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

/// Future transmitting the parameters and receiving the results of an invocation, see [`invoke`]
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

/// Invokes function `func` on instance `instance`, see [`invoke`]
///
/// This is like [`invoke`], but it only resolves once all I/O is done
pub async fn invoke_blocking<I, O, E, T, D>(
    tx: O,
    rx: I,
    instance: &str,
    func: &str,
    enc: E,
    params: T,
    dec: D,
) -> std::io::Result<D::Item>
where
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    E: Encoder<T, Error = std::io::Error> + Transmit,
    D: Decoder<Error = std::io::Error> + Receive,
{
    let (ret, io) = invoke(tx, rx, instance, func, enc, params, dec).await?;
    trace!("awaiting I/O completion");
    io.await?;
    Ok(ret)
}
