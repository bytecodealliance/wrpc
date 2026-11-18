//! wRPC connections

use core::mem;

use futures::TryStreamExt as _;
use tokio::io::AsyncRead;
use tokio_util::codec::FramedRead;

use crate::{Header, HeaderDecoder, error_connection_failed, frame};

enum Rx<I> {
    Header(FramedRead<I, HeaderDecoder>),
    Frames(FramedRead<I, frame::Decoder>),
    Failed,
}

/// Accepts the invocations sent over a byte stream pair one after another
pub struct Listener<I, O> {
    tx: O,
    rx: Rx<I>,
    header_decoder: HeaderDecoder,
    frame_decoder: frame::Decoder,
}

impl<I, O> Listener<I, O> {
    /// Constructs a new [Listener] over the byte streams `tx` and `rx`
    pub fn new(tx: O, rx: I) -> Self {
        Self::with_decoders(tx, rx, HeaderDecoder::default(), frame::Decoder::default())
    }

    /// Constructs a new [Listener] over the byte streams `tx` and `rx`, which decodes the
    /// invocation headers with `header` and their frames with `frames`
    pub fn with_decoders(tx: O, rx: I, header: HeaderDecoder, frames: frame::Decoder) -> Self {
        Self {
            tx,
            rx: Rx::Header(FramedRead::new(rx, header.clone())),
            header_decoder: header,
            frame_decoder: frames,
        }
    }

    /// Receives the header of the next invocation, once the previous one was served, and returns
    /// it along with `tx` and the frames read from `rx` to serve the invocation over, or `None`
    /// once `rx` reaches EOF
    ///
    /// This is cancel-safe, a partially received header is completed by the next call. Once
    /// receiving a header failed, this always fails. Once serving an invocation failed, the
    /// connection must be dropped, since its unread frames would be received as a header.
    pub async fn accept(
        &mut self,
    ) -> std::io::Result<Option<(Header, &mut O, &mut FramedRead<I, frame::Decoder>)>>
    where
        I: AsyncRead + Unpin,
    {
        self.rx = match mem::replace(&mut self.rx, Rx::Failed) {
            Rx::Frames(rx) => Rx::Header(rx.map_decoder(|_| self.header_decoder.clone())),
            rx => rx,
        };
        let Rx::Header(rx) = &mut self.rx else {
            return Err(error_connection_failed());
        };
        let header = match rx.try_next().await {
            Ok(Some(header)) => header,
            Ok(None) => return Ok(None),
            Err(err) => {
                self.rx = Rx::Failed;
                return Err(err.into());
            }
        };
        self.rx = match mem::replace(&mut self.rx, Rx::Failed) {
            Rx::Header(rx) => Rx::Frames(rx.map_decoder(|_| self.frame_decoder.clone())),
            rx => rx,
        };
        let Rx::Frames(rx) = &mut self.rx else {
            return Err(error_connection_failed());
        };
        Ok(Some((header, &mut self.tx, rx)))
    }
}
