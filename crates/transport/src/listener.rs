//! wRPC connections

use core::mem;

use futures::TryStreamExt as _;
use tokio::io::AsyncRead;
use tokio::sync::oneshot;
use tokio_util::codec::{FramedRead, FramedWrite};

use crate::{FrameDecoder, FrameEncoder, Header, HeaderDecoder, Server, error_connection_failed};

#[derive(Debug)]
enum Rx<I> {
    Header(FramedRead<I, HeaderDecoder>),
    Served(oneshot::Receiver<FramedRead<I, FrameDecoder>>),
    Failed,
}

/// Accepts the invocations sent over a byte stream pair one after another
#[derive(Debug)]
pub struct Listener<I, O> {
    rx: Rx<I>,
    tx_rx: oneshot::Receiver<FramedWrite<O, FrameEncoder>>,
    header_decoder: HeaderDecoder,
    frame_decoder: FrameDecoder,
}

impl<I, O> Listener<I, O> {
    /// Constructs a new [Listener] over the byte streams `tx` and `rx`
    pub fn new(tx: O, rx: I) -> Self {
        Self::with_decoders(tx, rx, HeaderDecoder::default(), FrameDecoder::default())
    }

    /// Constructs a new [Listener] over the byte streams `tx` and `rx`, which decodes the
    /// invocation headers with `header` and their frames with `frames`
    pub fn with_decoders(tx: O, rx: I, header: HeaderDecoder, frames: FrameDecoder) -> Self {
        let (tx_tx, tx_rx) = oneshot::channel();
        _ = tx_tx.send(FramedWrite::new(tx, FrameEncoder));
        Self {
            rx: Rx::Header(FramedRead::new(rx, header.clone())),
            tx_rx,
            header_decoder: header,
            frame_decoder: frames,
        }
    }

    /// Receives the header of the next invocation and returns it along with the [Server] serving
    /// it, or `None` once `rx` reaches EOF
    ///
    /// This pends until the future serving the previous invocation has received all of its
    /// parameters, so that future must be complete or polled concurrently with this, which
    /// pipelines the invocations. Their results are transmitted in the order they were accepted.
    /// The caller must bound how many invocations are in flight. A timeout around this is an idle
    /// timeout only when no invocation is in flight.
    ///
    /// This is cancel-safe, a partially received header is completed by the next call. This must
    /// not be called again once it or [`Server::serve`] failed. The invocations accepted after a
    /// failed one cannot transmit their results and fail with
    /// [`BrokenPipe`](std::io::ErrorKind::BrokenPipe).
    pub async fn accept(&mut self) -> std::io::Result<Option<(Header, Server<I, O>)>>
    where
        I: AsyncRead + Unpin,
    {
        if let Rx::Served(rx_rx) = &mut self.rx {
            self.rx = match rx_rx.await {
                Ok(rx) => Rx::Header(rx.map_decoder(|_| self.header_decoder.clone())),
                Err(..) => Rx::Failed,
            };
        }
        let Rx::Header(rx) = &mut self.rx else {
            return Err(error_connection_failed());
        };
        let header = match rx.try_next().await {
            Ok(Some(header)) => header,
            Ok(None) => return Ok(None),
            Err(err) => {
                self.rx = Rx::Failed;
                return Err(err);
            }
        };
        let (rx_tx, rx_rx) = oneshot::channel();
        let Rx::Header(rx) = mem::replace(&mut self.rx, Rx::Served(rx_rx)) else {
            return Err(error_connection_failed());
        };
        let (tx_tx, tx_rx) = oneshot::channel();
        Ok(Some((
            header,
            Server {
                tx_rx: mem::replace(&mut self.tx_rx, tx_rx),
                rx: rx.map_decoder(|_| self.frame_decoder.clone()),
                next_tx_tx: tx_tx,
                listener_rx_tx: rx_tx,
            },
        )))
    }
}
