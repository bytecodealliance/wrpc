use core::future::pending;
use core::marker::PhantomData;
use core::mem;
use core::pin::Pin;
use core::task::{Context, Poll, ready};

use std::collections::BTreeMap;
use std::vec;

use bytes::{Buf, BufMut as _, Bytes, BytesMut};
use futures::channel::mpsc;
use futures::stream;
use futures::{FutureExt as _, Stream, StreamExt as _};
use tokio::sync::oneshot;
use tokio_util::codec::{Decoder as _, Encoder as _};
use tracing::{error, instrument};
use wasm_tokio::cm::OptionDecoder;
use wasm_tokio::{CoreVecDecoderBytes, Leb128Encoder, put_u32_leb128};

use crate::value::{decode_buffered, encode_list};
use crate::{
    Decode, Encode, ListDecoder, Receive, ReceiveError, Transmit, encode_close, encode_frames,
};

/// Number of bytes or elements of a stream, after which encoding it eagerly stops
const MAX_READY: usize = u32::MAX as _;

fn encode_ready_bytes(chunks: &[impl AsRef<[u8]>], dst: &mut BytesMut) -> std::io::Result<()> {
    let n = chunks.iter().map(|chunk| chunk.as_ref().len()).sum();
    let len = u32::try_from(n)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
    dst.put_u8(0x01);
    Leb128Encoder.encode(len, dst)?;
    dst.reserve(n);
    for chunk in chunks {
        dst.extend_from_slice(chunk.as_ref());
    }
    Ok(())
}

fn encode_bytes_frames(path: &[usize], data: &[u8], dst: &mut BytesMut) -> std::io::Result<()> {
    let n = u32::try_from(data.len())
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
    let mut len = [0; 5];
    let len = put_u32_leb128(&mut len, n);
    encode_frames(path, Buf::chain(&*len, data), dst)
}

/// Encoder for `stream<T>` of type `S`, whose chunks are transmitted on its path and the values
/// nested in its elements below their indices
pub struct StreamEncoder<T: Encode, S> {
    items: Option<S>,
    /// Elements read, but not transmitted yet
    ready: Vec<T>,
    /// Index of the next element
    offset: usize,
    nested: BTreeMap<usize, T::Encoder>,
}

impl<T: Encode, S> Default for StreamEncoder<T, S> {
    fn default() -> Self {
        Self {
            items: None,
            ready: Vec::default(),
            offset: 0,
            nested: BTreeMap::default(),
        }
    }
}

impl<T, S> StreamEncoder<T, S>
where
    T: Encode,
    S: Stream<Item = Vec<T>> + Unpin,
{
    /// Returns the encoder of the pending `items`, whose encoding was written by the caller
    pub fn pending(items: S) -> Self {
        Self {
            items: Some(items),
            ..Self::default()
        }
    }
}

impl<T, S> Transmit for StreamEncoder<T, S>
where
    T: Encode,
    S: Stream<Item = Vec<T>> + Unpin,
{
    fn poll_transmit(
        &mut self,
        cx: &mut Context<'_>,
        path: &mut Vec<usize>,
        dst: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        while let Some(items) = &mut self.items {
            let next = if self.ready.is_empty() {
                items.poll_next_unpin(cx)
            } else {
                Poll::Ready(Some(mem::take(&mut self.ready)))
            };
            match next {
                Poll::Ready(Some(chunk)) if chunk.is_empty() => {}
                Poll::Ready(Some(chunk)) => {
                    let offset = self.offset;
                    self.offset = offset.checked_add(chunk.len()).ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "stream element index would overflow usize",
                        )
                    })?;
                    let mut buf = BytesMut::default();
                    let nested = encode_list(chunk, &mut buf)?;
                    let nested = nested
                        .into_iter()
                        .map(|(i, enc)| (offset.saturating_add(i), enc));
                    self.nested.extend(nested);
                    encode_frames(path, buf, dst)?;
                    break;
                }
                Poll::Ready(None) => {
                    self.items = None;
                    encode_close(path, dst)?;
                }
                Poll::Pending => break,
            }
        }
        ready!(self.nested.poll_transmit(cx, path, dst))?;
        if self.items.is_some() {
            return Poll::Pending;
        }
        Poll::Ready(Ok(()))
    }

    fn is_complete(&self) -> bool {
        self.items.is_none() && self.nested.is_complete()
    }
}

impl<T, S> tokio_util::codec::Encoder<S> for StreamEncoder<T, S>
where
    T: Encode,
    S: Stream<Item = Vec<T>> + Unpin,
{
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self, items), fields(ty = "stream"))]
    fn encode(&mut self, mut items: S, dst: &mut BytesMut) -> std::io::Result<()> {
        let mut ready = Vec::default();
        while ready.len() < MAX_READY {
            match items.next().now_or_never() {
                Some(Some(chunk)) => ready.extend(chunk),
                Some(None) => {
                    dst.put_u8(0x01);
                    let nested = encode_list(ready, dst)?;
                    *self = Self {
                        nested,
                        ..Self::default()
                    };
                    return Ok(());
                }
                None => break,
            }
        }
        dst.put_u8(0x00);
        *self = Self {
            ready,
            ..Self::pending(items)
        };
        Ok(())
    }
}

impl<T> Encode for Pin<Box<dyn Stream<Item = Vec<T>> + Send>>
where
    T: Encode + 'static,
{
    type Encoder = StreamEncoder<T, Self>;
}

impl<T> Encode for Pin<Box<dyn Stream<Item = Vec<T>>>>
where
    T: Encode + 'static,
{
    type Encoder = StreamEncoder<T, Self>;
}

/// Encoder for `stream<u8>` of type `T`, whose chunks are transmitted on its path
pub struct StreamEncoderBytes<T> {
    items: Option<T>,
    /// Chunks read, but not transmitted yet
    ready: vec::IntoIter<Bytes>,
}

impl<T> Default for StreamEncoderBytes<T> {
    fn default() -> Self {
        Self {
            items: None,
            ready: vec::IntoIter::default(),
        }
    }
}

impl<T> Transmit for StreamEncoderBytes<T>
where
    T: Stream<Item = Bytes> + Unpin,
{
    fn poll_transmit(
        &mut self,
        cx: &mut Context<'_>,
        path: &mut Vec<usize>,
        dst: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        while let Some(items) = &mut self.items {
            let next = match self.ready.next() {
                Some(chunk) => Some(chunk),
                None => ready!(items.poll_next_unpin(cx)),
            };
            match next {
                Some(chunk) if chunk.is_empty() => {}
                Some(chunk) => {
                    encode_bytes_frames(path, &chunk, dst)?;
                    return Poll::Pending;
                }
                None => {
                    self.items = None;
                    encode_close(path, dst)?;
                }
            }
        }
        Poll::Ready(Ok(()))
    }

    fn is_complete(&self) -> bool {
        self.items.is_none()
    }
}

impl<T> tokio_util::codec::Encoder<T> for StreamEncoderBytes<T>
where
    T: Stream<Item = Bytes> + Unpin,
{
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self, items), fields(ty = "stream<u8>"))]
    fn encode(&mut self, mut items: T, dst: &mut BytesMut) -> std::io::Result<()> {
        self.items = None;
        let mut ready = Vec::default();
        let mut n = 0;
        while n < MAX_READY {
            match items.next().now_or_never() {
                Some(Some(chunk)) => {
                    n = n.saturating_add(chunk.len());
                    ready.push(chunk);
                }
                Some(None) => return encode_ready_bytes(&ready, dst),
                None => break,
            }
        }
        dst.put_u8(0x00);
        self.items = Some(items);
        self.ready = ready.into_iter();
        Ok(())
    }
}

impl Encode for Pin<Box<dyn Stream<Item = Bytes> + Send>> {
    type Encoder = StreamEncoderBytes<Self>;
}

impl Encode for Pin<Box<dyn Stream<Item = Bytes>>> {
    type Encoder = StreamEncoderBytes<Self>;
}

fn receive_pending_stream<T>(
    rx: mpsc::Receiver<T>,
    end: oneshot::Receiver<()>,
) -> impl Stream<Item = T> {
    stream::unfold((rx, end), |(mut rx, end)| async move {
        if let Some(item) = rx.next().await {
            return Some((item, (rx, end)));
        }
        if end.await.is_err() {
            error!("stream I/O dropped");
            return pending().await;
        }
        None
    })
}

fn poll_receive_stream<D, T>(
    cx: &mut Context<'_>,
    tx: &mut Option<(mpsc::Sender<T>, oneshot::Sender<()>)>,
    dec: &mut D,
    buf: &mut BytesMut,
    src: &mut BytesMut,
    decode: impl FnOnce(&mut D, &mut BytesMut) -> std::io::Result<Option<T>>,
) -> Poll<Result<(), ReceiveError>>
where
    D: tokio_util::codec::Decoder<Error = std::io::Error>,
{
    if src.is_empty() {
        let Some((.., end)) = tx.take() else {
            return Poll::Ready(Err(ReceiveError::DataAfterEnd));
        };
        dec.decode_eof(buf)?;
        _ = end.send(());
        return Poll::Ready(Ok(()));
    }
    let Some((tx, ..)) = tx else {
        return Poll::Ready(Err(ReceiveError::DataAfterEnd));
    };
    ready!(tx.poll_ready(cx)).map_err(|_| ReceiveError::Unread)?;
    if let Some(chunk) = decode_buffered(buf, src, |buf| decode(dec, buf))? {
        tx.start_send(chunk).map_err(|_| ReceiveError::Unread)?;
    }
    Poll::Ready(Ok(()))
}

struct ChunkDecoder<T: Decode>(ListDecoder<T::Decoder>);

impl<T: Decode> Default for ChunkDecoder<T> {
    fn default() -> Self {
        Self(ListDecoder::default())
    }
}

impl<T: Decode> tokio_util::codec::Decoder for ChunkDecoder<T> {
    type Item = Vec<T>;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        T::decode_chunk(&mut self.0, src)
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match self.decode(src)? {
            Some(v) => Ok(Some(v)),
            None if self.0.cap == 0 && src.is_empty() => Ok(None),
            None => Err(std::io::ErrorKind::UnexpectedEof.into()),
        }
    }
}

/// Decoder for `stream<T>` of type `S`, whose chunks are received on its path and the values
/// nested in its elements below their indices
pub struct StreamDecoder<T, S = Pin<Box<dyn Stream<Item = Vec<T>> + Send>>>
where
    T: Decode,
{
    dec: OptionDecoder<ChunkDecoder<T>>,
    /// Sender of the chunks and of the end of a pending stream, until it is closed
    tx: Option<(mpsc::Sender<Vec<T>>, oneshot::Sender<()>)>,
    buffer: BytesMut,
    /// Index of the next element
    offset: usize,
    nested: BTreeMap<usize, T::Decoder>,
    _items: PhantomData<fn() -> S>,
}

impl<T, S> Default for StreamDecoder<T, S>
where
    T: Decode,
{
    fn default() -> Self {
        Self {
            dec: OptionDecoder::default(),
            tx: None,
            buffer: BytesMut::default(),
            offset: 0,
            nested: BTreeMap::default(),
            _items: PhantomData,
        }
    }
}

impl<T, S> StreamDecoder<T, S>
where
    T: Decode + 'static,
{
    /// Returns the decoder of a pending `stream<T>`, whose encoding was read by the caller, along
    /// with a [Stream] of its chunks
    pub fn pending() -> (Self, impl Stream<Item = Vec<T>> + 'static) {
        let (tx, rx) = mpsc::channel(0);
        let (end_tx, end_rx) = oneshot::channel();
        let dec = Self {
            tx: Some((tx, end_tx)),
            ..Self::default()
        };
        (dec, receive_pending_stream(rx, end_rx))
    }
}

impl<T, S> Receive for StreamDecoder<T, S>
where
    T: Decode,
{
    fn poll_receive(
        &mut self,
        cx: &mut Context<'_>,
        path: &[usize],
        src: &mut BytesMut,
    ) -> Poll<Result<(), ReceiveError>> {
        if !path.is_empty() {
            return self.nested.poll_receive(cx, path, src);
        }
        poll_receive_stream(
            cx,
            &mut self.tx,
            self.dec.get_mut(),
            &mut self.buffer,
            src,
            |dec, buf| {
                let mut chunk = Vec::default();
                while let Some(items) = dec.decode(buf)? {
                    let offset = self.offset;
                    self.offset = offset.checked_add(items.len()).ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "stream element index would overflow usize",
                        )
                    })?;
                    let nested = mem::take(&mut dec.0.nested)
                        .into_iter()
                        .map(|(i, dec)| (offset.saturating_add(i), dec));
                    self.nested.extend(nested);
                    chunk.extend(items);
                }
                Ok((!chunk.is_empty()).then_some(chunk))
            },
        )
    }

    fn is_complete(&self) -> bool {
        self.tx.is_none() && self.nested.is_complete()
    }
}

impl<T> tokio_util::codec::Decoder for StreamDecoder<T>
where
    T: Decode + Send + 'static,
{
    type Item = Pin<Box<dyn Stream<Item = Vec<T>> + Send>>;
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self), fields(ty = "stream"))]
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let Some(chunk) = self.dec.decode(src)? else {
            return Ok(None);
        };
        if let Some(chunk) = chunk {
            self.nested = mem::take(&mut self.dec.get_mut().0.nested);
            return Ok(Some(Box::pin(stream::iter(
                (!chunk.is_empty()).then_some(chunk),
            ))));
        }
        let (dec, rx) = Self::pending();
        *self = dec;
        Ok(Some(Box::pin(rx)))
    }
}

impl<T> tokio_util::codec::Decoder for StreamDecoder<T, Pin<Box<dyn Stream<Item = Vec<T>>>>>
where
    T: Decode + 'static,
{
    type Item = Pin<Box<dyn Stream<Item = Vec<T>>>>;
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self), fields(ty = "stream"))]
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let Some(chunk) = self.dec.decode(src)? else {
            return Ok(None);
        };
        if let Some(chunk) = chunk {
            self.nested = mem::take(&mut self.dec.get_mut().0.nested);
            return Ok(Some(Box::pin(stream::iter(
                (!chunk.is_empty()).then_some(chunk),
            ))));
        }
        let (dec, rx) = Self::pending();
        *self = dec;
        Ok(Some(Box::pin(rx)))
    }
}

impl<T> Decode for Pin<Box<dyn Stream<Item = Vec<T>> + Send>>
where
    T: Decode + Send + 'static,
{
    type Decoder = StreamDecoder<T>;
    type ListDecoder = ListDecoder<Self::Decoder>;
}

impl<T> Decode for Pin<Box<dyn Stream<Item = Vec<T>>>>
where
    T: Decode + 'static,
{
    type Decoder = StreamDecoder<T, Self>;
    type ListDecoder = ListDecoder<Self::Decoder>;
}

/// Decoder for `stream<u8>` of type `T`, whose chunks are received on its path
pub struct StreamDecoderBytes<T = Pin<Box<dyn Stream<Item = Bytes> + Send>>> {
    dec: OptionDecoder<CoreVecDecoderBytes>,
    /// Sender of the chunks and of the end of a pending stream, until it is closed
    tx: Option<(mpsc::Sender<Bytes>, oneshot::Sender<()>)>,
    buffer: BytesMut,
    _items: PhantomData<fn() -> T>,
}

impl<T> Default for StreamDecoderBytes<T> {
    fn default() -> Self {
        Self {
            dec: OptionDecoder::default(),
            tx: None,
            buffer: BytesMut::default(),
            _items: PhantomData,
        }
    }
}

impl<T> StreamDecoderBytes<T> {
    /// Returns the decoder of a pending `stream<u8>`, whose encoding was read by the caller, along
    /// with a [Stream] of its chunks
    pub fn pending() -> (Self, impl Stream<Item = Bytes> + 'static) {
        let (tx, rx) = mpsc::channel(0);
        let (end_tx, end_rx) = oneshot::channel();
        let dec = Self {
            tx: Some((tx, end_tx)),
            ..Self::default()
        };
        (dec, receive_pending_stream(rx, end_rx))
    }
}

impl<T> Receive for StreamDecoderBytes<T> {
    fn poll_receive(
        &mut self,
        cx: &mut Context<'_>,
        path: &[usize],
        src: &mut BytesMut,
    ) -> Poll<Result<(), ReceiveError>> {
        if !path.is_empty() {
            return Poll::Ready(Err(ReceiveError::UnknownPath));
        }
        poll_receive_stream(
            cx,
            &mut self.tx,
            self.dec.get_mut(),
            &mut self.buffer,
            src,
            |dec, buf| {
                let Some(mut chunk) = dec.decode(buf)? else {
                    return Ok(None);
                };
                if let Some(data) = dec.decode(buf)? {
                    let mut chunks = BytesMut::from(&chunk[..]);
                    chunks.extend_from_slice(&data);
                    while let Some(data) = dec.decode(buf)? {
                        chunks.extend_from_slice(&data);
                    }
                    chunk = chunks.freeze();
                }
                Ok((!chunk.is_empty()).then_some(chunk))
            },
        )
    }

    fn is_complete(&self) -> bool {
        self.tx.is_none()
    }
}

impl tokio_util::codec::Decoder for StreamDecoderBytes {
    type Item = Pin<Box<dyn Stream<Item = Bytes> + Send>>;
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self), fields(ty = "stream<u8>"))]
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let Some(chunk) = self.dec.decode(src)? else {
            return Ok(None);
        };
        if let Some(chunk) = chunk {
            return Ok(Some(Box::pin(stream::iter(
                (!chunk.is_empty()).then_some(chunk),
            ))));
        }
        let (dec, rx) = Self::pending();
        *self = dec;
        Ok(Some(Box::pin(rx)))
    }
}

impl tokio_util::codec::Decoder for StreamDecoderBytes<Pin<Box<dyn Stream<Item = Bytes>>>> {
    type Item = Pin<Box<dyn Stream<Item = Bytes>>>;
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self), fields(ty = "stream<u8>"))]
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let Some(chunk) = self.dec.decode(src)? else {
            return Ok(None);
        };
        if let Some(chunk) = chunk {
            return Ok(Some(Box::pin(stream::iter(
                (!chunk.is_empty()).then_some(chunk),
            ))));
        }
        let (dec, rx) = Self::pending();
        *self = dec;
        Ok(Some(Box::pin(rx)))
    }
}

impl Decode for Pin<Box<dyn Stream<Item = Bytes> + Send>> {
    type Decoder = StreamDecoderBytes;
    type ListDecoder = ListDecoder<Self::Decoder>;
}

impl Decode for Pin<Box<dyn Stream<Item = Bytes>>> {
    type Decoder = StreamDecoderBytes<Self>;
    type ListDecoder = ListDecoder<Self::Decoder>;
}
