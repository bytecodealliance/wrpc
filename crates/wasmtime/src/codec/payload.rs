use core::future::pending;
use core::task::{Context, Poll, ready};

use bytes::BytesMut;
use futures::stream;
use tokio::sync::{mpsc, oneshot};
use tokio_util::codec::Decoder;
use tokio_util::sync::PollSender;
use tracing::error;
use wasm_tokio::cm::{BoolCodec, F32Codec, F64Codec, S8Codec, U8Codec};
use wasm_tokio::{
    CoreNameDecoder, CoreVecDecoder, CoreVecDecoderBytes, Leb128DecoderI16, Leb128DecoderI32,
    Leb128DecoderI64, Leb128DecoderU16, Leb128DecoderU32, Leb128DecoderU64, Utf8Codec,
};
use wrpc_transport::{Receive, UnitCodec, error_unknown_path};

use crate::{FutureAny, StreamAny};

/// Chunks of a pending `stream` buffered for its reader
const STREAM_CHUNKS: usize = 128;

/// Decoder of `list<u8>` of at most `max_len` bytes
pub(super) struct ListDecoderU8(CoreVecDecoderBytes);

impl Decoder for ListDecoderU8 {
    type Item = Vec<u8>;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> std::io::Result<Option<Vec<u8>>> {
        let buf = self.0.decode(src)?;
        Ok(buf.map(Into::into))
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> std::io::Result<Option<Vec<u8>>> {
        let buf = self.0.decode_eof(src)?;
        Ok(buf.map(Into::into))
    }
}

/// Payload of a `future` or `stream`
pub(super) trait Payload: Sized + Send + 'static {
    type Decoder: Decoder<Item = Self, Error = std::io::Error> + Send + 'static;
    type ListDecoder: Decoder<Item = Vec<Self>, Error = std::io::Error> + Send + 'static;

    /// Returns a decoder rejecting values longer than `max_len`
    fn decoder(max_len: u32) -> Self::Decoder;

    /// Returns a decoder rejecting lists and their elements longer than `max_len`
    fn list_decoder(max_len: u32) -> Self::ListDecoder;
}

macro_rules! impl_payload {
    ($($t:ty => $dec:ident),+ $(,)?) => {
        $(
            impl Payload for $t {
                type Decoder = $dec;
                type ListDecoder = CoreVecDecoder<$dec>;

                fn decoder(_: u32) -> $dec {
                    $dec
                }

                fn list_decoder(max_len: u32) -> CoreVecDecoder<$dec> {
                    CoreVecDecoder::with_max_len($dec, max_len)
                }
            }
        )+
    };
}

impl_payload!(
    () => UnitCodec,
    bool => BoolCodec,
    i8 => S8Codec,
    i16 => Leb128DecoderI16,
    u16 => Leb128DecoderU16,
    i32 => Leb128DecoderI32,
    u32 => Leb128DecoderU32,
    i64 => Leb128DecoderI64,
    u64 => Leb128DecoderU64,
    f32 => F32Codec,
    f64 => F64Codec,
    char => Utf8Codec,
);

impl Payload for u8 {
    type Decoder = U8Codec;
    type ListDecoder = ListDecoderU8;

    fn decoder(_: u32) -> U8Codec {
        U8Codec
    }

    fn list_decoder(max_len: u32) -> ListDecoderU8 {
        ListDecoderU8(<CoreVecDecoderBytes>::with_max_len(max_len))
    }
}

impl Payload for String {
    type Decoder = CoreNameDecoder;
    type ListDecoder = CoreVecDecoder<CoreNameDecoder>;

    fn decoder(max_len: u32) -> CoreNameDecoder {
        <CoreNameDecoder>::with_max_len(max_len)
    }

    fn list_decoder(max_len: u32) -> CoreVecDecoder<CoreNameDecoder> {
        CoreVecDecoder::with_max_len(Self::decoder(max_len), max_len)
    }
}

fn error_data_after_end() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, "data received after end")
}

fn error_receiver_closed() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::BrokenPipe, "receiver closed")
}

/// Receives the value of a pending `future` on its path
struct FutureReceiver<P: Payload> {
    dec: P::Decoder,
    /// Sender of the value, until it is received
    tx: Option<oneshot::Sender<P>>,
    buf: BytesMut,
    closed: bool,
}

impl<P: Payload> Receive for FutureReceiver<P> {
    fn poll_receive(
        &mut self,
        _: &mut Context<'_>,
        path: &[usize],
        src: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        if !path.is_empty() {
            return Poll::Ready(Err(error_unknown_path(path)));
        }
        let Some(tx) = self.tx.take() else {
            if !src.is_empty() || self.closed {
                return Poll::Ready(Err(error_data_after_end()));
            }
            self.closed = true;
            return Poll::Ready(Ok(()));
        };
        let v = if src.is_empty() {
            self.closed = true;
            self.dec.decode_eof(&mut self.buf)
        } else {
            self.buf.unsplit(src.split());
            self.dec.decode(&mut self.buf)
        };
        let Some(v) = v? else {
            if self.closed {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "future closed without a value",
                )));
            }
            self.tx = Some(tx);
            return Poll::Ready(Ok(()));
        };
        if !self.buf.is_empty() {
            return Poll::Ready(Err(error_data_after_end()));
        }
        if tx.send(v).is_err() {
            return Poll::Ready(Err(error_receiver_closed()));
        }
        Poll::Ready(Ok(()))
    }

    fn is_closed(&self) -> bool {
        self.closed
    }
}

/// Returns a pending `future`, whose value of at most `max_len` is received by the receiver
pub(super) fn pending_future<P: Payload>(max_len: u32) -> (FutureAny, Box<dyn Receive + Send>) {
    let (tx, rx) = oneshot::channel();
    let r = FutureReceiver::<P> {
        dec: P::decoder(max_len),
        tx: Some(tx),
        buf: BytesMut::default(),
        closed: false,
    };
    let v = FutureAny::new(async {
        let Ok(v) = rx.await else {
            error!("future I/O dropped");
            return pending().await;
        };
        Ok(v)
    });
    (v, Box::new(r))
}

/// Receives the chunks of a pending `stream` on its path
struct StreamReceiver<P: Payload> {
    dec: P::ListDecoder,
    max_len: usize,
    /// Sender of the chunks and of the end, until the path is closed
    tx: Option<(PollSender<Vec<P>>, oneshot::Sender<()>)>,
    buf: BytesMut,
}

impl<P: Payload> Receive for StreamReceiver<P> {
    fn poll_receive(
        &mut self,
        cx: &mut Context<'_>,
        path: &[usize],
        src: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        if !path.is_empty() {
            return Poll::Ready(Err(error_unknown_path(path)));
        }
        if src.is_empty() {
            let Some((.., end)) = self.tx.take() else {
                return Poll::Ready(Err(error_data_after_end()));
            };
            self.dec.decode_eof(&mut self.buf)?;
            _ = end.send(());
            return Poll::Ready(Ok(()));
        }
        let Some((tx, ..)) = &mut self.tx else {
            return Poll::Ready(Err(error_data_after_end()));
        };
        ready!(tx.poll_reserve(cx)).map_err(|_| error_receiver_closed())?;
        self.buf.unsplit(src.split());
        let mut chunk = Vec::default();
        while let Some(items) = self.dec.decode(&mut self.buf)? {
            if chunk.len().saturating_add(items.len()) > self.max_len {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "stream chunk exceeds the maximum of `{}` elements",
                        self.max_len
                    ),
                )));
            }
            chunk.extend(items);
        }
        if !chunk.is_empty() {
            tx.send_item(chunk).map_err(|_| error_receiver_closed())?;
        }
        Poll::Ready(Ok(()))
    }

    fn is_closed(&self) -> bool {
        self.tx.is_none()
    }
}

/// Returns a pending `stream`, whose chunks of at most `max_len` elements, each at most `max_len`
/// long, are received by the receiver
pub(super) fn pending_stream<P: Payload>(max_len: u32) -> (StreamAny, Box<dyn Receive + Send>) {
    let (tx, rx) = mpsc::channel(STREAM_CHUNKS);
    let (end_tx, end_rx) = oneshot::channel();
    let r = StreamReceiver::<P> {
        dec: P::list_decoder(max_len),
        max_len: usize::try_from(max_len).unwrap_or(usize::MAX),
        tx: Some((PollSender::new(tx), end_tx)),
        buf: BytesMut::default(),
    };
    let items = stream::unfold((rx, end_rx), |(mut rx, end)| async {
        if let Some(chunk) = rx.recv().await {
            return Some((chunk, (rx, end)));
        }
        if end.await.is_err() {
            error!("stream I/O dropped");
            return pending().await;
        }
        None
    });
    (StreamAny::new(items), Box::new(r))
}
