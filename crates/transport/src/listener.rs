//! wRPC connections

use core::mem;

use futures::TryStreamExt as _;
use tokio::io::AsyncRead;
use tokio_util::codec::{FramedRead, FramedWrite};

use crate::{FrameDecoder, FrameEncoder, Framed, Header, HeaderDecoder, error_connection_failed};

enum State<I, O> {
    Header(FramedWrite<O, FrameEncoder>, FramedRead<I, HeaderDecoder>),
    Frames(Framed<I, O>),
    Failed,
}

/// Accepts the invocations sent over a byte stream pair one after another
pub struct Listener<I, O> {
    state: State<I, O>,
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
        Self {
            state: State::Header(
                FramedWrite::new(tx, FrameEncoder),
                FramedRead::new(rx, header.clone()),
            ),
            header_decoder: header,
            frame_decoder: frames,
        }
    }

    /// Receives the header of the next invocation, once the previous one was served, and returns
    /// it along with the frames to serve the invocation over, or `None` once `rx` reaches EOF
    ///
    /// This is cancel-safe, a partially received header is completed by the next call. Once
    /// receiving a header failed, this always fails. Once serving an invocation failed, the
    /// connection must be dropped, since its unread frames would be received as a header.
    pub async fn accept(&mut self) -> std::io::Result<Option<(Header, &mut Framed<I, O>)>>
    where
        I: AsyncRead + Unpin,
    {
        self.state = match mem::replace(&mut self.state, State::Failed) {
            State::Frames(Framed { tx, rx }) => {
                State::Header(tx, rx.map_decoder(|_| self.header_decoder.clone()))
            }
            state => state,
        };
        let State::Header(.., rx) = &mut self.state else {
            return Err(error_connection_failed());
        };
        let header = match rx.try_next().await {
            Ok(Some(header)) => header,
            Ok(None) => return Ok(None),
            Err(err) => {
                self.state = State::Failed;
                return Err(err);
            }
        };
        self.state = match mem::replace(&mut self.state, State::Failed) {
            State::Header(tx, rx) => State::Frames(Framed {
                tx,
                rx: rx.map_decoder(|_| self.frame_decoder.clone()),
            }),
            state => state,
        };
        let State::Frames(io) = &mut self.state else {
            return Err(error_connection_failed());
        };
        Ok(Some((header, io)))
    }
}
