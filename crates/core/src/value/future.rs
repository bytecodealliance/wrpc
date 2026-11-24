use core::future::{self, pending};
use core::marker::PhantomData;
use core::pin::Pin;
use core::task::{Context, Poll, ready};

use bytes::{BufMut as _, BytesMut};
use futures::FutureExt as _;
use tokio::sync::oneshot;
use tokio_util::codec::{Decoder as _, Encoder as _};
use tracing::{error, instrument};
use wasm_tokio::cm::OptionDecoder;

use crate::value::decode_buffered;
use crate::{
    Decode, Encode, ListDecoder, Receive, ReceiveError, Transmit, encode_close, encode_frames,
};

/// Encoder for `future<T>` of type `F`, whose value is transmitted on its path and the values
/// nested in it below `0`
pub struct FutureEncoder<T: Encode, F> {
    fut: Option<F>,
    nested: T::Encoder,
}

impl<T: Encode, F> Default for FutureEncoder<T, F> {
    fn default() -> Self {
        Self {
            fut: None,
            nested: T::Encoder::default(),
        }
    }
}

impl<T, F> FutureEncoder<T, F>
where
    T: Encode,
    F: Future<Output = T> + Unpin,
{
    /// Returns the encoder of the pending `fut`, whose encoding was written by the caller
    pub fn pending(fut: F) -> Self {
        Self {
            fut: Some(fut),
            nested: T::Encoder::default(),
        }
    }
}

impl<T, F> Transmit for FutureEncoder<T, F>
where
    T: Encode,
    F: Future<Output = T> + Unpin,
{
    fn poll_transmit(
        &mut self,
        cx: &mut Context<'_>,
        path: &mut Vec<usize>,
        dst: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        if let Some(fut) = &mut self.fut {
            let v = ready!(fut.poll_unpin(cx));
            self.fut = None;
            let mut buf = BytesMut::default();
            self.nested.encode(v, &mut buf)?;
            encode_frames(path, buf, dst)?;
            encode_close(path, dst)?;
        }
        path.push(0);
        let res = self.nested.poll_transmit(cx, path, dst);
        path.pop();
        res
    }

    fn is_complete(&self) -> bool {
        self.fut.is_none() && self.nested.is_complete()
    }
}

impl<T, F> tokio_util::codec::Encoder<F> for FutureEncoder<T, F>
where
    T: Encode,
    F: Future<Output = T> + Unpin,
{
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self, item), fields(ty = "future"))]
    fn encode(&mut self, mut item: F, dst: &mut BytesMut) -> std::io::Result<()> {
        if let Some(item) = (&mut item).now_or_never() {
            dst.put_u8(0x01);
            self.fut = None;
            self.nested.encode(item, dst)?;
            return Ok(());
        }
        dst.put_u8(0x00);
        self.fut = Some(item);
        Ok(())
    }
}

impl<T> Encode for Pin<Box<dyn Future<Output = T> + Send>>
where
    T: Encode + 'static,
{
    type Encoder = FutureEncoder<T, Self>;
}

impl<T> Encode for Pin<Box<dyn Future<Output = T>>>
where
    T: Encode + 'static,
{
    type Encoder = FutureEncoder<T, Self>;
}

/// Decoder for `future<T>` of type `F`, whose value is received on its path and the values nested
/// in it below `0`
pub struct FutureDecoder<T, F = Pin<Box<dyn Future<Output = T> + Send>>>
where
    T: Decode,
{
    dec: OptionDecoder<T::Decoder>,
    /// Sender of the value of a pending future, until it is received
    tx: Option<oneshot::Sender<T>>,
    buffer: BytesMut,
    closed: bool,
    _fut: PhantomData<fn() -> F>,
}

impl<T, F> Default for FutureDecoder<T, F>
where
    T: Decode,
{
    fn default() -> Self {
        Self {
            dec: OptionDecoder::default(),
            tx: None,
            buffer: BytesMut::default(),
            closed: true,
            _fut: PhantomData,
        }
    }
}

impl<T, F> FutureDecoder<T, F>
where
    T: Decode + 'static,
{
    /// Returns the decoder of a pending `future<T>`, whose encoding was read by the caller, along
    /// with a future, which resolves to its value once it is received
    pub fn pending() -> (Self, impl Future<Output = T> + 'static) {
        let (tx, rx) = oneshot::channel();
        let dec = Self {
            tx: Some(tx),
            closed: false,
            ..Self::default()
        };
        (dec, async {
            let Ok(v) = rx.await else {
                error!("future I/O dropped");
                return pending().await;
            };
            v
        })
    }
}

impl<T, F> Receive for FutureDecoder<T, F>
where
    T: Decode,
{
    fn poll_receive(
        &mut self,
        cx: &mut Context<'_>,
        path: &[usize],
        src: &mut BytesMut,
    ) -> Poll<Result<(), ReceiveError>> {
        if let Some((i, rest)) = path.split_first() {
            if *i != 0 || self.tx.is_some() {
                return Poll::Ready(Err(ReceiveError::UnknownPath));
            }
            return self.dec.poll_receive(cx, rest, src);
        }
        let Some(tx) = self.tx.take() else {
            if !src.is_empty() || self.closed {
                return Poll::Ready(Err(ReceiveError::DataAfterEnd));
            }
            self.closed = true;
            return Poll::Ready(Ok(()));
        };
        let v = if src.is_empty() {
            self.closed = true;
            self.dec.get_mut().decode_eof(&mut self.buffer)
        } else {
            let dec = self.dec.get_mut();
            decode_buffered(&mut self.buffer, src, |buf| dec.decode(buf))
        };
        let Some(v) = v? else {
            if self.closed {
                return Poll::Ready(Err(ReceiveError::Truncated));
            }
            self.tx = Some(tx);
            return Poll::Ready(Ok(()));
        };
        if !self.buffer.is_empty() {
            return Poll::Ready(Err(ReceiveError::DataAfterEnd));
        }
        if tx.send(v).is_err() {
            return Poll::Ready(Err(ReceiveError::Unread));
        }
        Poll::Ready(Ok(()))
    }

    fn is_complete(&self) -> bool {
        self.closed && self.dec.is_complete()
    }
}

impl<T> tokio_util::codec::Decoder for FutureDecoder<T>
where
    T: Decode + Send + 'static,
{
    type Item = Pin<Box<dyn Future<Output = T> + Send>>;
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self), fields(ty = "future"))]
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let Some(item) = self.dec.decode(src)? else {
            return Ok(None);
        };
        if let Some(item) = item {
            return Ok(Some(Box::pin(future::ready(item))));
        }
        let (dec, rx) = Self::pending();
        *self = dec;
        Ok(Some(Box::pin(rx)))
    }
}

impl<T> tokio_util::codec::Decoder for FutureDecoder<T, Pin<Box<dyn Future<Output = T>>>>
where
    T: Decode + 'static,
{
    type Item = Pin<Box<dyn Future<Output = T>>>;
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self), fields(ty = "future"))]
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let Some(item) = self.dec.decode(src)? else {
            return Ok(None);
        };
        if let Some(item) = item {
            return Ok(Some(Box::pin(future::ready(item))));
        }
        let (dec, rx) = Self::pending();
        *self = dec;
        Ok(Some(Box::pin(rx)))
    }
}

impl<T> Decode for Pin<Box<dyn Future<Output = T> + Send>>
where
    T: Decode + Send + 'static,
{
    type Decoder = FutureDecoder<T>;
    type ListDecoder = ListDecoder<Self::Decoder>;
}

impl<T> Decode for Pin<Box<dyn Future<Output = T>>>
where
    T: Decode + 'static,
{
    type Decoder = FutureDecoder<T, Self>;
    type ListDecoder = ListDecoder<Self::Decoder>;
}
