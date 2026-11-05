use core::any::TypeId;
use core::fmt::{self, Debug};
use core::future::pending;
use core::hash::{Hash, Hasher};
use core::iter::zip;
use core::marker::PhantomData;
use core::mem;
use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use core::str;

use bytes::{Buf as _, BufMut as _, Bytes, BytesMut};
use futures::stream::{self, FuturesUnordered};
use futures::{FutureExt as _, Stream, StreamExt as _, TryStreamExt as _};
use tokio::io::{AsyncBufRead, AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::select;
use tokio::sync::{mpsc, oneshot};
use tokio_util::codec::{Encoder as _, FramedParts, FramedRead};
use tokio_util::io::StreamReader;
use tracing::{Instrument as _, Span, error, instrument, trace};
use wasm_tokio::cm::{
    BoolCodec, F32Codec, F64Codec, OptionDecoder, OptionEncoder, PrimValEncoder, ResultDecoder,
    ResultEncoder, S8Codec, S16Codec, S32Codec, S64Codec, TupleDecoder, TupleEncoder, U8Codec,
    U16Codec, U32Codec, U64Codec,
};
use wasm_tokio::{
    CoreNameDecoder, CoreNameEncoder, CoreVecDecoder, CoreVecDecoderBytes, CoreVecEncoderBytes,
    DEFAULT_MAX_INITIAL_CAPACITY, Leb128DecoderI8, Leb128DecoderI16, Leb128DecoderI32,
    Leb128DecoderI64, Leb128DecoderI128, Leb128DecoderU8, Leb128DecoderU16, Leb128DecoderU32,
    Leb128DecoderU64, Leb128DecoderU128, Leb128Encoder, Utf8Codec,
};

use crate::frame::{Incoming, Outgoing};
use crate::{Index, encode_ready_bytes_header, error_receiver_closed, error_trailing_data};

/// Size of chunks read from byte streams
pub const READ_CHUNK_SIZE: usize = 8192;

const MAX_PENDING_STREAM_BATCH: usize = 1 << 20;

fn encode_ready_bytes<T: AsRef<[u8]>>(chunks: &[T], dst: &mut BytesMut) -> std::io::Result<()> {
    let n = chunks.iter().map(|chunk| chunk.as_ref().len()).sum();
    encode_ready_bytes_header(n, dst)?;
    dst.reserve(n);
    for chunk in chunks {
        dst.extend_from_slice(chunk.as_ref());
    }
    Ok(())
}

/// Borrowed resource handle, represented as an opaque byte blob
#[repr(transparent)]
pub struct ResourceBorrow<T: ?Sized> {
    repr: Bytes,
    _ty: PhantomData<T>,
}

impl<T: ?Sized> From<Bytes> for ResourceBorrow<T> {
    fn from(repr: Bytes) -> Self {
        Self {
            repr,
            _ty: PhantomData,
        }
    }
}

impl<T: ?Sized> From<Vec<u8>> for ResourceBorrow<T> {
    fn from(repr: Vec<u8>) -> Self {
        Self {
            repr: repr.into(),
            _ty: PhantomData,
        }
    }
}

impl<T: ?Sized> From<ResourceBorrow<T>> for Bytes {
    fn from(ResourceBorrow { repr, .. }: ResourceBorrow<T>) -> Self {
        repr
    }
}

impl<T: ?Sized> PartialEq for ResourceBorrow<T> {
    fn eq(&self, other: &Self) -> bool {
        self.repr == other.repr
    }
}

impl<T: ?Sized> Eq for ResourceBorrow<T> {}

impl<T: ?Sized> Hash for ResourceBorrow<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.repr.hash(state);
    }
}

impl<T: ?Sized> AsRef<[u8]> for ResourceBorrow<T> {
    fn as_ref(&self) -> &[u8] {
        &self.repr
    }
}

impl<T: ?Sized> AsRef<Bytes> for ResourceBorrow<T> {
    fn as_ref(&self) -> &Bytes {
        &self.repr
    }
}

impl<T: ?Sized + 'static> Debug for ResourceBorrow<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "borrow<{:?}>", TypeId::of::<T>())
    }
}

impl<T: ?Sized> Clone for ResourceBorrow<T> {
    fn clone(&self) -> Self {
        Self {
            repr: self.repr.clone(),
            _ty: PhantomData,
        }
    }
}

impl<T: ?Sized> ResourceBorrow<T> {
    /// Constructs a new borrowed resource handle
    pub fn new(repr: impl Into<Bytes>) -> Self {
        Self::from(repr.into())
    }
}

/// Owned resource handle, represented as an opaque byte blob
#[repr(transparent)]
pub struct ResourceOwn<T: ?Sized> {
    repr: Bytes,
    _ty: PhantomData<T>,
}

impl<T: ?Sized> From<ResourceOwn<T>> for ResourceBorrow<T> {
    fn from(ResourceOwn { repr, _ty }: ResourceOwn<T>) -> Self {
        Self {
            repr,
            _ty: PhantomData,
        }
    }
}

impl<T: ?Sized> From<Bytes> for ResourceOwn<T> {
    fn from(repr: Bytes) -> Self {
        Self {
            repr,
            _ty: PhantomData,
        }
    }
}

impl<T: ?Sized> From<Vec<u8>> for ResourceOwn<T> {
    fn from(repr: Vec<u8>) -> Self {
        Self {
            repr: repr.into(),
            _ty: PhantomData,
        }
    }
}

impl<T: ?Sized> From<ResourceOwn<T>> for Bytes {
    fn from(ResourceOwn { repr, .. }: ResourceOwn<T>) -> Self {
        repr
    }
}

impl<T: ?Sized> PartialEq for ResourceOwn<T> {
    fn eq(&self, other: &Self) -> bool {
        self.repr == other.repr
    }
}

impl<T: ?Sized> Eq for ResourceOwn<T> {}

impl<T: ?Sized> Hash for ResourceOwn<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.repr.hash(state);
    }
}

impl<T: ?Sized> AsRef<[u8]> for ResourceOwn<T> {
    fn as_ref(&self) -> &[u8] {
        &self.repr
    }
}

impl<T: ?Sized> AsRef<Bytes> for ResourceOwn<T> {
    fn as_ref(&self) -> &Bytes {
        &self.repr
    }
}

impl<T: ?Sized + 'static> Debug for ResourceOwn<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "own<{:?}>", TypeId::of::<T>())
    }
}

impl<T: ?Sized> Clone for ResourceOwn<T> {
    fn clone(&self) -> Self {
        Self {
            repr: self.repr.clone(),
            _ty: PhantomData,
        }
    }
}

impl<T: ?Sized> ResourceOwn<T> {
    /// Constructs a new owned resource handle
    pub fn new(repr: impl Into<Bytes>) -> Self {
        Self::from(repr.into())
    }

    /// Returns the owned handle as [`ResourceBorrow`]
    pub fn as_borrow(&self) -> ResourceBorrow<T> {
        ResourceBorrow {
            repr: self.repr.clone(),
            _ty: PhantomData,
        }
    }
}

/// Deferred operation used for async value processing
pub type DeferredFn<T> =
    Box<dyn FnOnce(T) -> Pin<Box<dyn Future<Output = std::io::Result<()>> + Send>> + Send>;

/// Handles async processing state for codecs
pub trait Deferred<T> {
    /// Takes a deferred async processing operation, if any
    fn take_deferred(&mut self) -> Option<DeferredFn<T>>;
}

macro_rules! impl_deferred_sync {
    ($t:ty) => {
        impl<T> Deferred<T> for $t {
            fn take_deferred(&mut self) -> Option<DeferredFn<T>> {
                None
            }
        }
    };
}

impl_deferred_sync!(BoolCodec);
impl_deferred_sync!(S8Codec);
impl_deferred_sync!(U8Codec);
impl_deferred_sync!(S16Codec);
impl_deferred_sync!(U16Codec);
impl_deferred_sync!(S32Codec);
impl_deferred_sync!(U32Codec);
impl_deferred_sync!(S64Codec);
impl_deferred_sync!(U64Codec);
impl_deferred_sync!(F32Codec);
impl_deferred_sync!(F64Codec);
impl_deferred_sync!(CoreNameDecoder);
impl_deferred_sync!(CoreNameEncoder);
impl_deferred_sync!(CoreVecDecoderBytes);
impl_deferred_sync!(CoreVecEncoderBytes);
impl_deferred_sync!(Utf8Codec);
impl_deferred_sync!(PrimValEncoder);
impl_deferred_sync!(Utf8BytesEncoder);
impl_deferred_sync!(Leb128Encoder);
impl_deferred_sync!(Leb128DecoderI8);
impl_deferred_sync!(Leb128DecoderU8);
impl_deferred_sync!(Leb128DecoderI16);
impl_deferred_sync!(Leb128DecoderU16);
impl_deferred_sync!(Leb128DecoderI32);
impl_deferred_sync!(Leb128DecoderU32);
impl_deferred_sync!(Leb128DecoderI64);
impl_deferred_sync!(Leb128DecoderU64);
impl_deferred_sync!(Leb128DecoderI128);
impl_deferred_sync!(Leb128DecoderU128);
impl_deferred_sync!(ResourceEncoder);
impl_deferred_sync!(UnitCodec);
impl_deferred_sync!(ListDecoderU8);

impl_deferred_sync!(CoreVecDecoder<BoolCodec>);
impl_deferred_sync!(CoreVecDecoder<S8Codec>);
impl_deferred_sync!(CoreVecDecoder<U8Codec>);
impl_deferred_sync!(CoreVecDecoder<S16Codec>);
impl_deferred_sync!(CoreVecDecoder<U16Codec>);
impl_deferred_sync!(CoreVecDecoder<S32Codec>);
impl_deferred_sync!(CoreVecDecoder<U32Codec>);
impl_deferred_sync!(CoreVecDecoder<S64Codec>);
impl_deferred_sync!(CoreVecDecoder<U64Codec>);
impl_deferred_sync!(CoreVecDecoder<F32Codec>);
impl_deferred_sync!(CoreVecDecoder<F64Codec>);
impl_deferred_sync!(CoreVecDecoder<CoreNameDecoder>);
impl_deferred_sync!(CoreVecDecoder<CoreVecDecoderBytes>);
impl_deferred_sync!(CoreVecDecoder<Utf8Codec>);
impl_deferred_sync!(CoreVecDecoder<Leb128DecoderI8>);
impl_deferred_sync!(CoreVecDecoder<Leb128DecoderU8>);
impl_deferred_sync!(CoreVecDecoder<Leb128DecoderI16>);
impl_deferred_sync!(CoreVecDecoder<Leb128DecoderU16>);
impl_deferred_sync!(CoreVecDecoder<Leb128DecoderI32>);
impl_deferred_sync!(CoreVecDecoder<Leb128DecoderU32>);
impl_deferred_sync!(CoreVecDecoder<Leb128DecoderI64>);
impl_deferred_sync!(CoreVecDecoder<Leb128DecoderU64>);
impl_deferred_sync!(CoreVecDecoder<Leb128DecoderI128>);
impl_deferred_sync!(CoreVecDecoder<Leb128DecoderU128>);
impl_deferred_sync!(CoreVecDecoder<UnitCodec>);

/// Codec for synchronous values
///
/// This is a wrapper struct, which provides a no-op [Deferred] implementation
/// for any codec.
pub struct SyncCodec<T>(pub T);

impl<T> Deref for SyncCodec<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for SyncCodec<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T, C> Deferred<T> for SyncCodec<C> {
    fn take_deferred(&mut self) -> Option<DeferredFn<T>> {
        None
    }
}

impl<T: Default> Default for SyncCodec<T> {
    fn default() -> Self {
        Self(T::default())
    }
}

impl<T, I> tokio_util::codec::Encoder<I> for SyncCodec<T>
where
    T: tokio_util::codec::Encoder<I>,
{
    type Error = T::Error;

    fn encode(&mut self, item: I, dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.0.encode(item, dst)
    }
}

impl<T> tokio_util::codec::Decoder for SyncCodec<T>
where
    T: tokio_util::codec::Decoder,
{
    type Item = T::Item;
    type Error = T::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        self.0.decode(src)
    }

    fn decode_eof(&mut self, buf: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        self.0.decode_eof(buf)
    }

    fn framed<IO: AsyncRead + AsyncWrite + Sized>(
        self,
        io: IO,
    ) -> tokio_util::codec::Framed<IO, Self>
    where
        Self: Sized,
    {
        self.0.framed(io).map_codec(Self)
    }
}

/// Source of deferred reception of asynchronous values, decoded by [Decode]
pub trait Source: Index + AsyncRead + Send + Sync + Unpin + 'static {
    /// Returns `bytes` read from this source to it, so that they are read again before the rest
    /// of its data
    fn unread(&mut self, bytes: BytesMut);

    /// Returns a future, which resolves to the result of the invocation once it ends
    fn closed(&self) -> impl Future<Output = std::io::Result<()>> + Send + 'static + use<Self>;
}

impl<T> Source for Incoming<T>
where
    T: AsyncBufRead + Send + Unpin + 'static,
{
    fn unread(&mut self, bytes: BytesMut) {
        Self::unread(self, bytes);
    }

    fn closed(&self) -> impl Future<Output = std::io::Result<()>> + Send + 'static + use<T> {
        Self::closed(self)
    }
}

/// Target of deferred transmission of asynchronous values, encoded by [Encode]
///
/// `offset` is the position of the byte written by the encoder of the asynchronous value
/// in the buffer the value was encoded into.
pub trait Sink: Index + Send + Sync + 'static {
    /// Transmits the value of a `future<T>`
    fn write_future<T>(
        self,
        offset: usize,
        item: impl Future<Output = T> + Send + 'static,
    ) -> impl Future<Output = std::io::Result<()>> + Send
    where
        T: Encode<Self>,
        std::io::Error: From<<T::Encoder as tokio_util::codec::Encoder<T>>::Error>;

    /// Transmits the elements of a `stream<T>`
    fn write_stream<T>(
        self,
        offset: usize,
        items: impl Stream<Item = Vec<T>> + Send + Unpin + 'static,
    ) -> impl Future<Output = std::io::Result<()>> + Send
    where
        T: Encode<Self> + Send + 'static,
        std::io::Error: From<<T::Encoder as tokio_util::codec::Encoder<T>>::Error>;

    /// Transmits the elements of a `stream<u8>` from a [Stream] of [Bytes]
    fn write_stream_bytes(
        self,
        offset: usize,
        items: impl Stream<Item = Bytes> + Send + Unpin + 'static,
    ) -> impl Future<Output = std::io::Result<()>> + Send;

    /// Transmits the elements of a `stream<u8>` from `ready` followed by an [AsyncRead]
    fn write_stream_read(
        self,
        offset: usize,
        ready: BytesMut,
        items: impl AsyncRead + Send + Unpin + 'static,
    ) -> impl Future<Output = std::io::Result<()>> + Send;
}

impl<W> Sink for Outgoing<W>
where
    W: AsyncWrite + Send + Unpin + 'static,
{
    async fn write_future<T>(
        mut self,
        _offset: usize,
        item: impl Future<Output = T> + Send + 'static,
    ) -> std::io::Result<()>
    where
        T: Encode<Self>,
        std::io::Error: From<<T::Encoder as tokio_util::codec::Encoder<T>>::Error>,
    {
        let item = item.await;
        let mut enc = T::Encoder::default();
        let mut buf = BytesMut::default();
        enc.encode(item, &mut buf)?;
        self.write_all(&buf).await?;
        if let Some(f) = enc.take_deferred() {
            f(self.nested()).await?;
        }
        self.shutdown().await
    }

    async fn write_stream<T>(
        mut self,
        _offset: usize,
        mut items: impl Stream<Item = Vec<T>> + Send + Unpin + 'static,
    ) -> std::io::Result<()>
    where
        T: Encode<Self> + Send + 'static,
        std::io::Error: From<<T::Encoder as tokio_util::codec::Encoder<T>>::Error>,
    {
        let mut enc = T::Encoder::default();
        let mut buf = BytesMut::default();
        let mut deferred = Vec::default();
        let mut tasks = FuturesUnordered::default();
        let mut pending = false;
        let mut ended = false;
        let mut closed = false;
        let mut i = 0_u64;
        loop {
            if closed && tasks.is_empty() {
                return Ok(());
            }
            select! {
                chunk = items.next(), if !ended && buf.len() < MAX_PENDING_STREAM_BATCH => {
                    let Some(chunk) = chunk else {
                        trace!("writing stream end");
                        ended = true;
                        continue;
                    };
                    if chunk.is_empty() {
                        continue;
                    }
                    let n = u32::try_from(chunk.len()).map_err(|err| {
                        std::io::Error::new(std::io::ErrorKind::InvalidInput, err)
                    })?;
                    let end = i.checked_add(n.into()).ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "stream element index would overflow u64",
                        )
                    })?;
                    trace!(n, "encoding chunk length");
                    Leb128Encoder.encode(n, &mut buf)?;
                    trace!(i, buf = format!("{buf:02x?}"), "writing stream chunk items");

                    buf.reserve(chunk.len());
                    for (i, item) in zip(i.., chunk) {
                        enc.encode(item, &mut buf)?;
                        if let Some(f) = enc.take_deferred() {
                            let i = usize::try_from(i)
                                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
                            let w = self.index(&[i])?;
                            deferred.push(f(w));
                        }
                    }
                    i = end;
                }
                Some(res) = tasks.next() => {
                    trace!(?res, "receiver task finished");
                    res?;
                }
                res = async {
                    if !buf.is_empty() {
                        trace!(?buf, "writing bytes from buffer");
                        self.write(&buf).await
                    } else if ended {
                        trace!("closing stream");
                        self.shutdown().await.map(|()| 0)
                    } else {
                        self.flush().await.map(|()| 0)
                    }
                }, if pending || !buf.is_empty() || (ended && !closed) => {
                    let n = res?;
                    if buf.is_empty() {
                        pending = false;
                        closed = ended;
                        tasks.extend(deferred.drain(..));
                        continue;
                    }
                    buf.advance(n);
                    if buf.is_empty() {
                        pending = true;
                        tasks.extend(deferred.drain(..));
                    }
                }
            }
        }
    }

    async fn write_stream_bytes(
        mut self,
        _offset: usize,
        mut items: impl Stream<Item = Bytes> + Send + Unpin + 'static,
    ) -> std::io::Result<()> {
        let mut buf = BytesMut::default();
        let mut pending = false;
        loop {
            select! {
                chunk = items.next(), if buf.len() < MAX_PENDING_STREAM_BATCH => {
                    let Some(chunk) = chunk else {
                        trace!("writing stream end");
                        self.write_all(&buf).await?;
                        return self.shutdown().await
                    };
                    if chunk.is_empty() {
                        continue;
                    }
                    let n = u32::try_from(chunk.len()).map_err(|err| {
                        std::io::Error::new(std::io::ErrorKind::InvalidInput, err)
                    })?;
                    trace!(n, "encoding chunk length");
                    Leb128Encoder.encode(n, &mut buf)?;
                    buf.extend_from_slice(&chunk);
                }
                res = self.flush(), if pending || !buf.is_empty() => {
                    res?;
                    pending = false;
                    if !buf.is_empty() {
                        self.write_all(&buf).await?;
                        buf.clear();
                        pending = true;
                    }
                }
            }
        }
    }

    async fn write_stream_read(
        mut self,
        _offset: usize,
        mut buf: BytesMut,
        mut items: impl AsyncRead + Send + Unpin + 'static,
    ) -> std::io::Result<()> {
        if !buf.is_empty() {
            let n = u32::try_from(buf.len())
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
            trace!(n, "writing ready chunk length");
            let mut header = BytesMut::default();
            Leb128Encoder.encode(n, &mut header)?;
            self.write_all(&header).await?;
        }
        let mut chunk = BytesMut::with_capacity(READ_CHUNK_SIZE);
        let mut pending = false;
        loop {
            select! {
                res = items.read_buf(&mut chunk), if buf.len() < MAX_PENDING_STREAM_BATCH => {
                    let n = res?;
                    if n == 0 {
                        trace!("writing stream end");
                        self.write_all(&buf).await?;
                        return self.shutdown().await
                    }
                    let n = u32::try_from(n).map_err(|err| {
                        std::io::Error::new(std::io::ErrorKind::InvalidInput, err)
                    })?;
                    trace!(n, "encoding chunk length");
                    Leb128Encoder.encode(n, &mut buf)?;
                    buf.extend_from_slice(&chunk);
                    chunk.clear();
                }
                res = self.flush(), if pending || !buf.is_empty() => {
                    res?;
                    pending = false;
                    if !buf.is_empty() {
                        self.write_all(&buf).await?;
                        buf.clear();
                        pending = true;
                    }
                }
            }
        }
    }
}

#[instrument(level = "trace", skip(w, deferred))]
async fn handle_deferred<T, I>(w: &T, deferred: I) -> std::io::Result<()>
where
    T: Index,
    I: IntoIterator<Item = Option<DeferredFn<T>>>,
    I::IntoIter: ExactSizeIterator,
{
    let mut futs = FuturesUnordered::default();
    for (i, f) in zip(0.., deferred) {
        if let Some(f) = f {
            let w = w.index(&[i])?;
            futs.push(f(w));
        }
    }
    while let Some(()) = futs.try_next().await? {}
    Ok(())
}

/// Defines value encoding
pub trait Encode<W: Sink>: Sized {
    /// Encoder used to encode the value
    type Encoder: tokio_util::codec::Encoder<Self> + Deferred<W> + Default + Send;

    /// Convenience function for encoding a value
    #[instrument(level = "trace", skip(self, enc))]
    fn encode(
        self,
        enc: &mut Self::Encoder,
        dst: &mut BytesMut,
    ) -> Result<Option<DeferredFn<W>>, <Self::Encoder as tokio_util::codec::Encoder<Self>>::Error>
    {
        enc.encode(self, dst)?;
        Ok(enc.take_deferred())
    }

    /// Encode an iterator of owned values
    #[instrument(level = "trace", skip(items, enc))]
    fn encode_iter_own<I>(
        items: I,
        enc: &mut Self::Encoder,
        dst: &mut BytesMut,
    ) -> Result<Option<DeferredFn<W>>, <Self::Encoder as tokio_util::codec::Encoder<Self>>::Error>
    where
        I: IntoIterator<Item = Self>,
        I::IntoIter: ExactSizeIterator,
    {
        let items = items.into_iter();
        dst.reserve(items.len());
        let mut deferred = Vec::with_capacity(items.len());
        for item in items {
            enc.encode(item, dst)?;
            deferred.push(enc.take_deferred());
        }
        if deferred.iter().any(Option::is_some) {
            Ok(Some(Box::new(move |w| {
                Box::pin(async move { handle_deferred(&w, deferred).await })
            })))
        } else {
            Ok(None)
        }
    }

    /// Encode an iterator of value references
    #[instrument(level = "trace", skip(items, enc))]
    fn encode_iter_ref<'a, I>(
        items: I,
        enc: &mut Self::Encoder,
        dst: &mut BytesMut,
    ) -> Result<Option<DeferredFn<W>>, <Self::Encoder as tokio_util::codec::Encoder<&'a Self>>::Error>
    where
        I: IntoIterator<Item = &'a Self>,
        I::IntoIter: ExactSizeIterator,
        Self::Encoder: tokio_util::codec::Encoder<&'a Self>,
    {
        let items = items.into_iter();
        dst.reserve(items.len());
        let mut deferred = Vec::with_capacity(items.len());
        for item in items {
            enc.encode(item, dst)?;
            deferred.push(enc.take_deferred());
        }
        if deferred.iter().any(Option::is_some) {
            Ok(Some(Box::new(move |w| {
                Box::pin(async move { handle_deferred(&w, deferred).await })
            })))
        } else {
            Ok(None)
        }
    }

    /// Encode a list of owned values
    #[instrument(level = "trace", skip(items, enc), fields(ty = "list"))]
    fn encode_list_own(
        items: Vec<Self>,
        enc: &mut Self::Encoder,
        dst: &mut BytesMut,
    ) -> Result<Option<DeferredFn<W>>, <Self::Encoder as tokio_util::codec::Encoder<Self>>::Error>
    {
        let n = u32::try_from(items.len())
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
        dst.reserve(5 + items.len());
        Leb128Encoder.encode(n, dst)?;
        Self::encode_iter_own(items, enc, dst)
    }

    /// Encode a list of value references
    #[instrument(level = "trace", skip(items, enc), fields(ty = "list"))]
    fn encode_list_ref<'a>(
        items: &'a [Self],
        enc: &mut Self::Encoder,
        dst: &mut BytesMut,
    ) -> Result<Option<DeferredFn<W>>, <Self::Encoder as tokio_util::codec::Encoder<&'a Self>>::Error>
    where
        Self::Encoder: tokio_util::codec::Encoder<&'a Self>,
    {
        let n = u32::try_from(items.len())
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
        dst.reserve(5 + items.len());
        Leb128Encoder.encode(n, dst)?;
        Self::encode_iter_ref(items, enc, dst)
    }
}

/// Defines value decoding
pub trait Decode<R: Source>: Sized {
    /// Decoder used to decode value
    type Decoder: tokio_util::codec::Decoder<Item = Self> + Deferred<R> + Default + Send + 'static;
    /// Decoder used to decode lists of value
    type ListDecoder: tokio_util::codec::Decoder<Item = Vec<Self>> + Default + 'static;
}

impl<T, W> Deferred<W> for OptionEncoder<T>
where
    T: Deferred<W>,
{
    fn take_deferred(&mut self) -> Option<DeferredFn<W>> {
        self.0.take_deferred()
    }
}

impl<T, W: Sink> Encode<W> for Option<T>
where
    T: Encode<W>,
{
    type Encoder = OptionEncoder<T::Encoder>;
}

impl<'a, T, W: Sink> Encode<W> for &'a Option<T>
where
    T: Encode<W>,
    T::Encoder: tokio_util::codec::Encoder<&'a T>,
{
    type Encoder = OptionEncoder<T::Encoder>;
}

impl<T, W> Deferred<W> for OptionDecoder<T>
where
    T: Deferred<W> + Default,
{
    fn take_deferred(&mut self) -> Option<DeferredFn<W>> {
        mem::take(self).into_inner().take_deferred()
    }
}

impl<T, R: Source> Decode<R> for Option<T>
where
    T: Decode<R>,
{
    type Decoder = OptionDecoder<T::Decoder>;
    type ListDecoder = ListDecoder<Self::Decoder, R>;
}

impl<O, E, W> Deferred<W> for ResultEncoder<O, E>
where
    O: Deferred<W>,
    E: Deferred<W>,
{
    fn take_deferred(&mut self) -> Option<DeferredFn<W>> {
        match (self.ok.take_deferred(), self.err.take_deferred()) {
            (None, None) => None,
            (Some(ok), None) => Some(ok),
            (None, Some(err)) => Some(err),
            (Some(ok), Some(_)) => {
                if cfg!(debug_assertions) {
                    panic!("both `result::ok` and `result::err` deferred function set");
                } else {
                    Some(ok)
                }
            }
        }
    }
}

impl<O, E, W: Sink> Encode<W> for Result<O, E>
where
    O: Encode<W>,
    E: Encode<W>,
    std::io::Error: From<<O::Encoder as tokio_util::codec::Encoder<O>>::Error>,
    std::io::Error: From<<E::Encoder as tokio_util::codec::Encoder<E>>::Error>,
{
    type Encoder = ResultEncoder<O::Encoder, E::Encoder>;
}

impl<'a, O, E, W: Sink> Encode<W> for &'a Result<O, E>
where
    O: Encode<W>,
    O::Encoder: tokio_util::codec::Encoder<&'a O>,
    E: Encode<W>,
    E::Encoder: tokio_util::codec::Encoder<&'a E>,
    std::io::Error: From<<O::Encoder as tokio_util::codec::Encoder<&'a O>>::Error>,
    std::io::Error: From<<E::Encoder as tokio_util::codec::Encoder<&'a E>>::Error>,
{
    type Encoder = ResultEncoder<O::Encoder, E::Encoder>;
}

impl<O, E, W> Deferred<W> for ResultDecoder<O, E>
where
    O: Deferred<W> + Default,
    E: Deferred<W> + Default,
{
    fn take_deferred(&mut self) -> Option<DeferredFn<W>> {
        let (mut ok, mut err) = mem::take(self).into_inner();
        match (ok.take_deferred(), err.take_deferred()) {
            (None, None) => None,
            (Some(ok), None) => Some(ok),
            (None, Some(err)) => Some(err),
            (Some(ok), Some(_)) => {
                if cfg!(debug_assertions) {
                    panic!("both `result::ok` and `result::err` deferred function set");
                } else {
                    Some(ok)
                }
            }
        }
    }
}

impl<O, E, R: Source> Decode<R> for Result<O, E>
where
    O: Decode<R>,
    E: Decode<R>,
    std::io::Error: From<<O::Decoder as tokio_util::codec::Decoder>::Error>,
    std::io::Error: From<<E::Decoder as tokio_util::codec::Decoder>::Error>,
{
    type Decoder = ResultDecoder<O::Decoder, E::Decoder>;
    type ListDecoder = ListDecoder<Self::Decoder, R>;
}

/// Encoder for `list<T>`
pub struct ListEncoder<W> {
    deferred: Option<DeferredFn<W>>,
}

impl<W> Default for ListEncoder<W> {
    fn default() -> Self {
        Self { deferred: None }
    }
}

impl<W> Deferred<W> for ListEncoder<W> {
    fn take_deferred(&mut self) -> Option<DeferredFn<W>> {
        self.deferred.take()
    }
}

impl<T, W: Sink> tokio_util::codec::Encoder<Vec<T>> for ListEncoder<W>
where
    T: Encode<W>,
{
    type Error = <T::Encoder as tokio_util::codec::Encoder<T>>::Error;

    fn encode(&mut self, items: Vec<T>, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let mut enc = T::Encoder::default();
        self.deferred = T::encode_list_own(items, &mut enc, dst)?;
        Ok(())
    }
}

impl<'a, T, W: Sink> tokio_util::codec::Encoder<&'a Vec<T>> for ListEncoder<W>
where
    T: Encode<W>,
    T::Encoder: tokio_util::codec::Encoder<&'a T>,
{
    type Error = <T::Encoder as tokio_util::codec::Encoder<&'a T>>::Error;

    fn encode(&mut self, items: &'a Vec<T>, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let mut enc = T::Encoder::default();
        self.deferred = T::encode_list_ref(items, &mut enc, dst)?;
        Ok(())
    }
}

impl<'a, 'b, T, W: Sink> tokio_util::codec::Encoder<&'a &'b Vec<T>> for ListEncoder<W>
where
    T: Encode<W>,
    T::Encoder: tokio_util::codec::Encoder<&'b T>,
{
    type Error = <T::Encoder as tokio_util::codec::Encoder<&'b T>>::Error;

    fn encode(&mut self, items: &'a &'b Vec<T>, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let mut enc = T::Encoder::default();
        self.deferred = T::encode_list_ref(items, &mut enc, dst)?;
        Ok(())
    }
}

impl<'a, T, W: Sink> tokio_util::codec::Encoder<&'a [T]> for ListEncoder<W>
where
    T: Encode<W>,
    T::Encoder: tokio_util::codec::Encoder<&'a T>,
{
    type Error = <T::Encoder as tokio_util::codec::Encoder<&'a T>>::Error;

    fn encode(&mut self, items: &'a [T], dst: &mut BytesMut) -> Result<(), Self::Error> {
        let mut enc = T::Encoder::default();
        self.deferred = T::encode_list_ref(items, &mut enc, dst)?;
        Ok(())
    }
}

impl<'a, 'b, T, W: Sink> tokio_util::codec::Encoder<&'a &'b [T]> for ListEncoder<W>
where
    T: Encode<W>,
    T::Encoder: tokio_util::codec::Encoder<&'b T>,
{
    type Error = <T::Encoder as tokio_util::codec::Encoder<&'b T>>::Error;

    fn encode(&mut self, items: &'a &'b [T], dst: &mut BytesMut) -> Result<(), Self::Error> {
        let mut enc = T::Encoder::default();
        self.deferred = T::encode_list_ref(items, &mut enc, dst)?;
        Ok(())
    }
}

impl<T, W: Sink> Encode<W> for Vec<T>
where
    T: Encode<W>,
{
    type Encoder = ListEncoder<W>;
}

impl<'a, T, W: Sink> Encode<W> for &'a Vec<T>
where
    T: Encode<W>,
    T::Encoder: tokio_util::codec::Encoder<&'a T>,
{
    type Encoder = ListEncoder<W>;
}

impl<'a, T, W: Sink> Encode<W> for &'a [T]
where
    T: Encode<W>,
    T::Encoder: tokio_util::codec::Encoder<&'a T>,
{
    type Encoder = ListEncoder<W>;
}

/// Decoder for `list<T>`
pub struct ListDecoder<T, R>
where
    T: tokio_util::codec::Decoder,
{
    dec: T,
    ret: Vec<T::Item>,
    cap: usize,
    deferred: Vec<Option<DeferredFn<R>>>,
}

impl<T, R> ListDecoder<T, R>
where
    T: tokio_util::codec::Decoder,
{
    /// Constructs a new list decoder
    pub fn new(dec: T) -> Self {
        Self {
            dec,
            ret: Vec::default(),
            cap: 0,
            deferred: vec![],
        }
    }
}

impl<T, R: Source> Default for ListDecoder<T, R>
where
    T: tokio_util::codec::Decoder + Default,
{
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T, R: Source> Deferred<R> for ListDecoder<T, R>
where
    T: tokio_util::codec::Decoder,
{
    fn take_deferred(&mut self) -> Option<DeferredFn<R>> {
        let deferred = mem::take(&mut self.deferred);
        if deferred.iter().any(Option::is_some) {
            Some(Box::new(|r| {
                Box::pin(async move { handle_deferred(&r, deferred).await })
            }))
        } else {
            None
        }
    }
}

impl<T, R: Source> tokio_util::codec::Decoder for ListDecoder<T, R>
where
    T: tokio_util::codec::Decoder + Deferred<R>,
{
    type Item = Vec<T::Item>;
    type Error = T::Error;

    #[instrument(level = "trace", skip(self), fields(ty = "list"))]
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if self.cap == 0 {
            let Some(len) = Leb128DecoderU32.decode(src)? else {
                return Ok(None);
            };
            if len == 0 {
                return Ok(Some(Vec::default()));
            }
            let len = usize::try_from(len)
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
            self.ret = Vec::with_capacity(
                len.min(DEFAULT_MAX_INITIAL_CAPACITY / mem::size_of::<T::Item>().max(1)),
            );
            self.cap = len;
        }
        while self.cap > 0 {
            let v = match self.dec.decode(src) {
                Ok(Some(v)) => v,
                Ok(None) => return Ok(None),
                Err(err) => {
                    self.cap = 0;
                    self.ret.clear();
                    self.deferred.clear();
                    return Err(err);
                }
            };
            if let Some(deferred) = self.dec.take_deferred() {
                self.deferred.resize_with(self.ret.len(), || None);
                self.deferred.push(Some(deferred));
            }
            self.ret.push(v);
            self.cap -= 1;
        }
        Ok(Some(mem::take(&mut self.ret)))
    }
}

impl<T, R: Source> Decode<R> for Vec<T>
where
    T: Decode<R> + Send,
    T::ListDecoder: Deferred<R> + Send,
{
    type Decoder = T::ListDecoder;
    type ListDecoder = ListDecoder<Self::Decoder, R>;
}

/// Encoder for `list<T, N>`
pub struct ArrayEncoder<W> {
    deferred: Option<DeferredFn<W>>,
}

impl<W> Default for ArrayEncoder<W> {
    fn default() -> Self {
        Self { deferred: None }
    }
}

impl<W> Deferred<W> for ArrayEncoder<W> {
    fn take_deferred(&mut self) -> Option<DeferredFn<W>> {
        self.deferred.take()
    }
}

impl<T, W: Sink, const N: usize> tokio_util::codec::Encoder<[T; N]> for ArrayEncoder<W>
where
    T: Encode<W>,
{
    type Error = <T::Encoder as tokio_util::codec::Encoder<T>>::Error;

    #[instrument(level = "trace", skip(self, items), fields(ty = "array"))]
    fn encode(&mut self, items: [T; N], dst: &mut BytesMut) -> Result<(), Self::Error> {
        let mut enc = T::Encoder::default();
        self.deferred = T::encode_iter_own(items, &mut enc, dst)?;
        Ok(())
    }
}

impl<'a, T, W: Sink, const N: usize> tokio_util::codec::Encoder<&'a [T; N]> for ArrayEncoder<W>
where
    T: Encode<W>,
    T::Encoder: tokio_util::codec::Encoder<&'a T>,
{
    type Error = <T::Encoder as tokio_util::codec::Encoder<&'a T>>::Error;

    #[instrument(level = "trace", skip(self, items), fields(ty = "array"))]
    fn encode(&mut self, items: &'a [T; N], dst: &mut BytesMut) -> Result<(), Self::Error> {
        let mut enc = T::Encoder::default();
        self.deferred = T::encode_iter_ref(items, &mut enc, dst)?;
        Ok(())
    }
}

impl<'a, 'b, T, W: Sink, const N: usize> tokio_util::codec::Encoder<&'a &'b [T; N]>
    for ArrayEncoder<W>
where
    T: Encode<W>,
    T::Encoder: tokio_util::codec::Encoder<&'b T>,
{
    type Error = <T::Encoder as tokio_util::codec::Encoder<&'b T>>::Error;

    #[instrument(level = "trace", skip(self, items), fields(ty = "array"))]
    fn encode(&mut self, items: &'a &'b [T; N], dst: &mut BytesMut) -> Result<(), Self::Error> {
        let mut enc = T::Encoder::default();
        self.deferred = T::encode_iter_ref(*items, &mut enc, dst)?;
        Ok(())
    }
}

impl<T, W: Sink, const N: usize> Encode<W> for [T; N]
where
    T: Encode<W>,
{
    type Encoder = ArrayEncoder<W>;
}

impl<'a, T, W: Sink, const N: usize> Encode<W> for &'a [T; N]
where
    T: Encode<W>,
    T::Encoder: tokio_util::codec::Encoder<&'a T>,
{
    type Encoder = ArrayEncoder<W>;
}

/// Decoder for `list<T, N>`
pub struct ArrayDecoder<T, R, const N: usize>
where
    T: tokio_util::codec::Decoder,
{
    dec: T,
    ret: Vec<T::Item>,
    deferred: Vec<Option<DeferredFn<R>>>,
}

impl<T, R: Source, const N: usize> Default for ArrayDecoder<T, R, N>
where
    T: tokio_util::codec::Decoder + Default,
{
    fn default() -> Self {
        Self {
            dec: T::default(),
            ret: Vec::default(),
            deferred: Vec::default(),
        }
    }
}

impl<T, R: Source, const N: usize> Deferred<R> for ArrayDecoder<T, R, N>
where
    T: tokio_util::codec::Decoder,
{
    fn take_deferred(&mut self) -> Option<DeferredFn<R>> {
        let deferred = mem::take(&mut self.deferred);
        if deferred.iter().any(Option::is_some) {
            Some(Box::new(|r| {
                Box::pin(async move { handle_deferred(&r, deferred).await })
            }))
        } else {
            None
        }
    }
}

impl<T, R: Source, const N: usize> tokio_util::codec::Decoder for ArrayDecoder<T, R, N>
where
    T: tokio_util::codec::Decoder + Deferred<R>,
{
    type Item = [T::Item; N];
    type Error = T::Error;

    #[instrument(level = "trace", skip(self), fields(ty = "array"))]
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if self.ret.is_empty() {
            self.ret
                .reserve(N.min(DEFAULT_MAX_INITIAL_CAPACITY / mem::size_of::<T::Item>().max(1)));
        }
        while self.ret.len() < N {
            let v = match self.dec.decode(src) {
                Ok(Some(v)) => v,
                Ok(None) => return Ok(None),
                Err(err) => {
                    self.ret = Vec::default();
                    self.deferred.clear();
                    return Err(err);
                }
            };
            if let Some(deferred) = self.dec.take_deferred() {
                self.deferred.resize_with(self.ret.len(), || None);
                self.deferred.push(Some(deferred));
            }
            self.ret.push(v);
        }
        let Ok(ret) = mem::take(&mut self.ret).try_into() else {
            unreachable!("array decoder decoded exactly `N` elements")
        };
        Ok(Some(ret))
    }
}

impl<T, const N: usize, R: Source> Decode<R> for [T; N]
where
    T: Decode<R> + Send,
{
    type Decoder = ArrayDecoder<T::Decoder, R, N>;
    type ListDecoder = ListDecoder<Self::Decoder, R>;
}

macro_rules! impl_copy_codec {
    ($t:ty, $c:tt) => {
        impl<W: Sink> Encode<W> for $t {
            type Encoder = $c;

            #[instrument(level = "trace", skip(items))]
            fn encode_iter_own<I>(
                items: I,
                enc: &mut Self::Encoder,
                dst: &mut BytesMut,
            ) -> Result<
                Option<DeferredFn<W>>,
                <Self::Encoder as tokio_util::codec::Encoder<Self>>::Error,
            >
            where
                I: IntoIterator<Item = Self>,
                I::IntoIter: ExactSizeIterator,
            {
                let items = items.into_iter();
                dst.reserve(items.len());
                for item in items {
                    enc.encode(item, dst)?;
                }
                Ok(None)
            }

            #[instrument(level = "trace", skip(items))]
            fn encode_iter_ref<'a, I>(
                items: I,
                enc: &mut Self::Encoder,
                dst: &mut BytesMut,
            ) -> Result<
                Option<DeferredFn<W>>,
                <Self::Encoder as tokio_util::codec::Encoder<&'a Self>>::Error,
            >
            where
                I: IntoIterator<Item = &'a Self>,
                I::IntoIter: ExactSizeIterator,
            {
                let items = items.into_iter();
                dst.reserve(items.len());
                for item in items {
                    enc.encode(*item, dst)?;
                }
                Ok(None)
            }
        }

        impl<'b, W: Sink> Encode<W> for &'b $t {
            type Encoder = $c;

            #[instrument(level = "trace", skip(items))]
            fn encode_iter_own<I>(
                items: I,
                enc: &mut Self::Encoder,
                dst: &mut BytesMut,
            ) -> Result<
                Option<DeferredFn<W>>,
                <Self::Encoder as tokio_util::codec::Encoder<Self>>::Error,
            >
            where
                I: IntoIterator<Item = Self>,
                I::IntoIter: ExactSizeIterator,
            {
                let items = items.into_iter();
                dst.reserve(items.len());
                for item in items {
                    enc.encode(*item, dst)?;
                }
                Ok(None)
            }

            #[instrument(level = "trace", skip(items))]
            fn encode_iter_ref<'a, I>(
                items: I,
                enc: &mut Self::Encoder,
                dst: &mut BytesMut,
            ) -> Result<
                Option<DeferredFn<W>>,
                <Self::Encoder as tokio_util::codec::Encoder<&'a Self>>::Error,
            >
            where
                I: IntoIterator<Item = &'a Self>,
                I::IntoIter: ExactSizeIterator,
                'b: 'a,
            {
                let items = items.into_iter();
                dst.reserve(items.len());
                for item in items {
                    enc.encode(item, dst)?;
                }
                Ok(None)
            }
        }

        impl<R: Source> Decode<R> for $t {
            type Decoder = $c;
            type ListDecoder = CoreVecDecoder<Self::Decoder>;
        }
    };
}

impl_copy_codec!(bool, BoolCodec);
impl_copy_codec!(i8, S8Codec);
impl_copy_codec!(i16, S16Codec);
impl_copy_codec!(u16, U16Codec);
impl_copy_codec!(i32, S32Codec);
impl_copy_codec!(u32, U32Codec);
impl_copy_codec!(i64, S64Codec);
impl_copy_codec!(u64, U64Codec);
impl_copy_codec!(f32, F32Codec);
impl_copy_codec!(f64, F64Codec);
impl_copy_codec!(char, Utf8Codec);

impl<W: Sink> Encode<W> for u8 {
    type Encoder = U8Codec;

    #[instrument(level = "trace", skip(items))]
    fn encode_iter_own<I>(
        items: I,
        enc: &mut Self::Encoder,
        dst: &mut BytesMut,
    ) -> Result<Option<DeferredFn<W>>, <Self::Encoder as tokio_util::codec::Encoder<Self>>::Error>
    where
        I: IntoIterator<Item = Self>,
        I::IntoIter: ExactSizeIterator,
    {
        let items = items.into_iter();
        dst.reserve(items.len());
        dst.extend(items);
        Ok(None)
    }

    #[instrument(level = "trace", skip(items))]
    fn encode_iter_ref<'a, I>(
        items: I,
        enc: &mut Self::Encoder,
        dst: &mut BytesMut,
    ) -> Result<Option<DeferredFn<W>>, <Self::Encoder as tokio_util::codec::Encoder<&'a Self>>::Error>
    where
        I: IntoIterator<Item = &'a Self>,
        I::IntoIter: ExactSizeIterator,
    {
        let items = items.into_iter();
        dst.reserve(items.len());
        dst.extend(items);
        Ok(None)
    }

    #[instrument(level = "trace", skip(items), fields(ty = "list<u8>"))]
    fn encode_list_own(
        items: Vec<Self>,
        enc: &mut Self::Encoder,
        dst: &mut BytesMut,
    ) -> Result<Option<DeferredFn<W>>, <Self::Encoder as tokio_util::codec::Encoder<Self>>::Error>
    {
        CoreVecEncoderBytes.encode(items, dst)?;
        Ok(None)
    }

    #[instrument(level = "trace", skip(items), fields(ty = "list<u8>"))]
    fn encode_list_ref<'a>(
        items: &'a [Self],
        enc: &mut Self::Encoder,
        dst: &mut BytesMut,
    ) -> Result<Option<DeferredFn<W>>, <Self::Encoder as tokio_util::codec::Encoder<&'a Self>>::Error>
    where
        Self::Encoder: tokio_util::codec::Encoder<&'a Self>,
    {
        CoreVecEncoderBytes.encode(items, dst)?;
        Ok(None)
    }
}

impl<'b, W: Sink> Encode<W> for &'b u8 {
    type Encoder = U8Codec;

    #[instrument(level = "trace", skip(items))]
    fn encode_iter_own<I>(
        items: I,
        enc: &mut Self::Encoder,
        dst: &mut BytesMut,
    ) -> Result<Option<DeferredFn<W>>, <Self::Encoder as tokio_util::codec::Encoder<Self>>::Error>
    where
        I: IntoIterator<Item = Self>,
        I::IntoIter: ExactSizeIterator,
    {
        let items = items.into_iter();
        dst.reserve(items.len());
        dst.extend(items);
        Ok(None)
    }

    #[instrument(level = "trace", skip(items))]
    fn encode_iter_ref<'a, I>(
        items: I,
        enc: &mut Self::Encoder,
        dst: &mut BytesMut,
    ) -> Result<Option<DeferredFn<W>>, <Self::Encoder as tokio_util::codec::Encoder<&'a Self>>::Error>
    where
        I: IntoIterator<Item = &'a Self>,
        I::IntoIter: ExactSizeIterator,
        'b: 'a,
    {
        let items = items.into_iter();
        dst.reserve(items.len());
        dst.extend(items.map(|b| **b));
        Ok(None)
    }
}

/// Decoder for `list<u8>`
#[derive(Debug, Default)]
#[repr(transparent)]
pub struct ListDecoderU8(CoreVecDecoderBytes);

impl tokio_util::codec::Decoder for ListDecoderU8 {
    type Item = Vec<u8>;
    type Error = <CoreVecDecoderBytes as tokio_util::codec::Decoder>::Error;

    #[instrument(level = "trace", skip(self), fields(ty = "list<u8>"))]
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let Some(buf) = self.0.decode(src)? else {
            return Ok(None);
        };
        Ok(Some(buf.into()))
    }
}

impl<R: Source> Decode<R> for u8 {
    type Decoder = U8Codec;
    type ListDecoder = ListDecoderU8;
}

impl<W: Sink> Encode<W> for &str {
    type Encoder = CoreNameEncoder;
}

impl<W: Sink> Encode<W> for &&str {
    type Encoder = CoreNameEncoder;
}

impl<W: Sink> Encode<W> for String {
    type Encoder = CoreNameEncoder;
}

impl<W: Sink> Encode<W> for &String {
    type Encoder = CoreNameEncoder;
}

impl<R: Source> Decode<R> for String {
    type Decoder = CoreNameDecoder;
    type ListDecoder = CoreVecDecoder<Self::Decoder>;
}

/// Bytes encoded as a `string`, validated to be UTF-8 when encoded
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(transparent)]
pub struct Utf8Bytes<T>(pub T);

/// Encoder for [`Utf8Bytes`]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Utf8BytesEncoder;

impl<T: AsRef<[u8]>> tokio_util::codec::Encoder<Utf8Bytes<T>> for Utf8BytesEncoder {
    type Error = std::io::Error;

    #[instrument(level = "trace", skip_all, fields(ty = "string"))]
    fn encode(&mut self, item: Utf8Bytes<T>, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let Utf8Bytes(item) = item;
        let item = str::from_utf8(item.as_ref())
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        CoreNameEncoder.encode(item, dst)
    }
}

impl<T: AsRef<[u8]>, W: Sink> Encode<W> for Utf8Bytes<T> {
    type Encoder = Utf8BytesEncoder;
}

impl<W: Sink> Encode<W> for Bytes {
    type Encoder = CoreVecEncoderBytes;
}

impl<W: Sink> Encode<W> for &Bytes {
    type Encoder = CoreVecEncoderBytes;
}

impl<R: Source> Decode<R> for Bytes {
    type Decoder = CoreVecDecoderBytes;
    type ListDecoder = CoreVecDecoder<Self::Decoder>;
}

/// Encoder for `resource` types
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
#[repr(transparent)]
pub struct ResourceEncoder;

impl<T: ?Sized> tokio_util::codec::Encoder<ResourceOwn<T>> for ResourceEncoder {
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self, item), ret, fields(ty = "own"))]
    fn encode(&mut self, item: ResourceOwn<T>, dst: &mut BytesMut) -> std::io::Result<()> {
        CoreVecEncoderBytes.encode(item.repr, dst)
    }
}

impl<T: ?Sized> tokio_util::codec::Encoder<&ResourceOwn<T>> for ResourceEncoder {
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self, item), ret, fields(ty = "own"))]
    fn encode(&mut self, item: &ResourceOwn<T>, dst: &mut BytesMut) -> std::io::Result<()> {
        CoreVecEncoderBytes.encode(&item.repr, dst)
    }
}

impl<T: ?Sized, W: Sink> Encode<W> for ResourceOwn<T> {
    type Encoder = ResourceEncoder;
}

impl<T: ?Sized, W: Sink> Encode<W> for &ResourceOwn<T> {
    type Encoder = ResourceEncoder;
}

impl<T: ?Sized> tokio_util::codec::Encoder<ResourceBorrow<T>> for ResourceEncoder {
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self, item), ret, fields(ty = "borrow"))]
    fn encode(&mut self, item: ResourceBorrow<T>, dst: &mut BytesMut) -> std::io::Result<()> {
        CoreVecEncoderBytes.encode(item.repr, dst)
    }
}

impl<T: ?Sized> tokio_util::codec::Encoder<&ResourceBorrow<T>> for ResourceEncoder {
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self, item), ret, fields(ty = "borrow"))]
    fn encode(&mut self, item: &ResourceBorrow<T>, dst: &mut BytesMut) -> std::io::Result<()> {
        CoreVecEncoderBytes.encode(&item.repr, dst)
    }
}

impl<T: ?Sized, W: Sink> Encode<W> for ResourceBorrow<T> {
    type Encoder = ResourceEncoder;
}

impl<T: ?Sized, W: Sink> Encode<W> for &ResourceBorrow<T> {
    type Encoder = ResourceEncoder;
}

/// Decoder for borrowed resource types
#[derive(Debug)]
#[repr(transparent)]
pub struct ResourceBorrowDecoder<T: ?Sized> {
    dec: CoreVecDecoderBytes,
    _ty: PhantomData<T>,
}

impl<T: ?Sized> Default for ResourceBorrowDecoder<T> {
    fn default() -> Self {
        Self {
            dec: CoreVecDecoderBytes::default(),
            _ty: PhantomData,
        }
    }
}

impl<T: ?Sized, R> Deferred<R> for ResourceBorrowDecoder<T> {
    fn take_deferred(&mut self) -> Option<DeferredFn<R>> {
        None
    }
}

impl<T: ?Sized, R> Deferred<R> for CoreVecDecoder<ResourceBorrowDecoder<T>> {
    fn take_deferred(&mut self) -> Option<DeferredFn<R>> {
        None
    }
}

impl<T: ?Sized + Send + 'static, R: Source> Decode<R> for ResourceBorrow<T> {
    type Decoder = ResourceBorrowDecoder<T>;
    type ListDecoder = CoreVecDecoder<Self::Decoder>;
}

impl<T: ?Sized> tokio_util::codec::Decoder for ResourceBorrowDecoder<T> {
    type Item = ResourceBorrow<T>;
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self), fields(ty = "borrow"))]
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let repr = self.dec.decode(src)?;
        Ok(repr.map(Self::Item::from))
    }
}

/// Decoder for owned resource types
#[derive(Debug)]
#[repr(transparent)]
pub struct ResourceOwnDecoder<T: ?Sized> {
    dec: CoreVecDecoderBytes,
    _ty: PhantomData<T>,
}

impl<T: ?Sized> Default for ResourceOwnDecoder<T> {
    fn default() -> Self {
        Self {
            dec: CoreVecDecoderBytes::default(),
            _ty: PhantomData,
        }
    }
}

impl<T: ?Sized, R> Deferred<R> for ResourceOwnDecoder<T> {
    fn take_deferred(&mut self) -> Option<DeferredFn<R>> {
        None
    }
}

impl<T: ?Sized, R> Deferred<R> for CoreVecDecoder<ResourceOwnDecoder<T>> {
    fn take_deferred(&mut self) -> Option<DeferredFn<R>> {
        None
    }
}

impl<T: ?Sized + Send + 'static, R: Source> Decode<R> for ResourceOwn<T> {
    type Decoder = ResourceOwnDecoder<T>;
    type ListDecoder = CoreVecDecoder<Self::Decoder>;
}

impl<T: ?Sized> tokio_util::codec::Decoder for ResourceOwnDecoder<T> {
    type Item = ResourceOwn<T>;
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self), fields(ty = "own"))]
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let repr = self.dec.decode(src)?;
        Ok(repr.map(Self::Item::from))
    }
}

/// Codec for `()`
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
#[repr(transparent)]
pub struct UnitCodec;

impl tokio_util::codec::Encoder<()> for UnitCodec {
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self), ret)]
    fn encode(&mut self, (): (), dst: &mut BytesMut) -> std::io::Result<()> {
        Ok(())
    }
}

impl tokio_util::codec::Encoder<&()> for UnitCodec {
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self), ret)]
    fn encode(&mut self, (): &(), dst: &mut BytesMut) -> std::io::Result<()> {
        Ok(())
    }
}

impl tokio_util::codec::Decoder for UnitCodec {
    type Item = ();
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self))]
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        Ok(Some(()))
    }
}

/// Marker trait for [Encode] tuple types
pub trait TupleEncode<W: Sink>: Encode<W> {}

/// Marker trait for [Decode] tuple types
pub trait TupleDecode<R: Source>: Decode<R> {}

impl<W: Sink> Encode<W> for () {
    type Encoder = UnitCodec;
}

impl<W: Sink> TupleEncode<W> for () {}

impl<R: Source> Decode<R> for () {
    type Decoder = UnitCodec;
    type ListDecoder = CoreVecDecoder<Self::Decoder>;
}

impl<R: Source> TupleDecode<R> for () {}

macro_rules! impl_tuple_codec {
    ($($vn:ident),+; $($vt:ident),+; $($cn:ident),+; $($ct:ident),+) => {
        impl<W: Sink, $($ct),+> Deferred<W> for TupleEncoder::<($($ct),+,)>
        where
            $($ct: Deferred<W> + Default + 'static),+
        {
            fn take_deferred(&mut self) -> Option<DeferredFn<W>> {
                let Self(($(mut $cn),+,)) = mem::take(self);
                let deferred = [ $($cn.take_deferred()),+ ];
                if deferred.iter().any(Option::is_some) {
                    Some(Box::new(|w| {
                        Box::pin(async move { handle_deferred(&w, deferred).await })
                    }))
                } else {
                    None
                }
            }
        }

        impl<E, W: Sink, $($vt),+> Encode<W> for ($($vt),+,)
        where
            E: From<std::io::Error>,
            $(
                $vt: Encode<W>,
                $vt::Encoder: tokio_util::codec::Encoder<$vt, Error = E> + 'static,
            )+
        {
            type Encoder = TupleEncoder::<($($vt::Encoder),+,)>;
        }

        impl<E, W: Sink, $($vt),+> TupleEncode<W> for ($($vt),+,)
        where
            E: From<std::io::Error>,
            $(
                $vt: Encode<W>,
                $vt::Encoder: tokio_util::codec::Encoder<$vt, Error = E> + 'static,
            )+
        {
        }

        impl<'a, E, W: Sink, $($vt),+> Encode<W> for &'a ($($vt),+,)
        where
            E: From<std::io::Error>,
            $(
                $vt: Encode<W>,
                $vt::Encoder: tokio_util::codec::Encoder<&'a $vt, Error = E> + 'static,
            )+
        {
            type Encoder = TupleEncoder::<($($vt::Encoder),+,)>;
        }

        impl<R: Source, $($vt),+> Deferred<R> for TupleDecoder::<($($vt::Decoder),+,), ($(Option<$vt>),+,)>
        where
            $($vt: Decode<R>),+
        {
            fn take_deferred(&mut self) -> Option<DeferredFn<R>> {
                let ($(mut $cn),+,) = mem::take(self).into_inner();
                let deferred = [ $($cn.take_deferred()),+ ];
                if deferred.iter().any(Option::is_some) {
                    Some(Box::new(|r| {
                        Box::pin(async move { handle_deferred(&r, deferred).await })
                    }))
                } else {
                    None
                }
            }
        }

        impl<E, R: Source, $($vt),+> Decode<R> for ($($vt),+,)
        where
            E: From<std::io::Error>,
            $(
                $vt: Decode<R> + Send + 'static,
                $vt::Decoder: tokio_util::codec::Decoder<Error = E> + Send + 'static,
            )+
        {
            type Decoder = TupleDecoder::<($($vt::Decoder),+,), ($(Option<$vt>),+,)>;
            type ListDecoder = ListDecoder<Self::Decoder, R>;
        }

        impl<E, R: Source, $($vt),+> TupleDecode<R> for ($($vt),+,)
        where
            E: From<std::io::Error>,
            $(
                $vt: Decode<R> + Send + 'static,
                $vt::Decoder: tokio_util::codec::Decoder<Error = E> + Send + 'static,
            )+
        {
        }
    };
}

impl_tuple_codec!(
    v0;
    V0;
    c0;
    C0
);

impl_tuple_codec!(
    v0, v1;
    V0, V1;
    c0, c1;
    C0, C1
);

impl_tuple_codec!(
    v0, v1, v2;
    V0, V1, V2;
    c0, c1, c2;
    C0, C1, C2
);

impl_tuple_codec!(
    v0, v1, v2, v3;
    V0, V1, V2, V3;
    c0, c1, c2, c3;
    C0, C1, C2, C3
);

impl_tuple_codec!(
    v0, v1, v2, v3, v4;
    V0, V1, V2, V3, V4;
    c0, c1, c2, c3, c4;
    C0, C1, C2, C3, C4
);

impl_tuple_codec!(
    v0, v1, v2, v3, v4, v5;
    V0, V1, V2, V3, V4, V5;
    c0, c1, c2, c3, c4, c5;
    C0, C1, C2, C3, C4, C5
);

impl_tuple_codec!(
    v0, v1, v2, v3, v4, v5, v6;
    V0, V1, V2, V3, V4, V5, V6;
    c0, c1, c2, c3, c4, c5, c6;
    C0, C1, C2, C3, C4, C5, C6
);

impl_tuple_codec!(
    v0, v1, v2, v3, v4, v5, v6, v7;
    V0, V1, V2, V3, V4, V5, V6, V7;
    c0, c1, c2, c3, c4, c5, c6, c7;
    C0, C1, C2, C3, C4, C5, C6, C7
);

impl_tuple_codec!(
    v0, v1, v2, v3, v4, v5, v6, v7, v8;
    V0, V1, V2, V3, V4, V5, V6, V7, V8;
    c0, c1, c2, c3, c4, c5, c6, c7, c8;
    C0, C1, C2, C3, C4, C5, C6, C7, C8
);

impl_tuple_codec!(
    v0, v1, v2, v3, v4, v5, v6, v7, v8, v9;
    V0, V1, V2, V3, V4, V5, V6, V7, V8, V9;
    c0, c1, c2, c3, c4, c5, c6, c7, c8, c9;
    C0, C1, C2, C3, C4, C5, C6, C7, C8, C9
);

impl_tuple_codec!(
    v0, v1, v2, v3, v4, v5, v6, v7, v8, v9, v10;
    V0, V1, V2, V3, V4, V5, V6, V7, V8, V9, V10;
    c0, c1, c2, c3, c4, c5, c6, c7, c8, c9, c10;
    C0, C1, C2, C3, C4, C5, C6, C7, C8, C9, C10
);

impl_tuple_codec!(
    v0, v1, v2, v3, v4, v5, v6, v7, v8, v9, v10, v11;
    V0, V1, V2, V3, V4, V5, V6, V7, V8, V9, V10, V11;
    c0, c1, c2, c3, c4, c5, c6, c7, c8, c9, c10, c11;
    C0, C1, C2, C3, C4, C5, C6, C7, C8, C9, C10, C11
);

impl_tuple_codec!(
    v0, v1, v2, v3, v4, v5, v6, v7, v8, v9, v10, v11, v12;
    V0, V1, V2, V3, V4, V5, V6, V7, V8, V9, V10, V11, V12;
    c0, c1, c2, c3, c4, c5, c6, c7, c8, c9, c10, c11, c12;
    C0, C1, C2, C3, C4, C5, C6, C7, C8, C9, C10, C11, C12
);

impl_tuple_codec!(
    v0, v1, v2, v3, v4, v5, v6, v7, v8, v9, v10, v11, v12, v13;
    V0, V1, V2, V3, V4, V5, V6, V7, V8, V9, V10, V11, V12, V13;
    c0, c1, c2, c3, c4, c5, c6, c7, c8, c9, c10, c11, c12, c13;
    C0, C1, C2, C3, C4, C5, C6, C7, C8, C9, C10, C11, C12, C13
);

impl_tuple_codec!(
    v0, v1, v2, v3, v4, v5, v6, v7, v8, v9, v10, v11, v12, v13, v14;
    V0, V1, V2, V3, V4, V5, V6, V7, V8, V9, V10, V11, V12, V13, V14;
    c0, c1, c2, c3, c4, c5, c6, c7, c8, c9, c10, c11, c12, c13, c14;
    C0, C1, C2, C3, C4, C5, C6, C7, C8, C9, C10, C11, C12, C13, C14
);

impl_tuple_codec!(
    v0, v1, v2, v3, v4, v5, v6, v7, v8, v9, v10, v11, v12, v13, v14, v15;
    V0, V1, V2, V3, V4, V5, V6, V7, V8, V9, V10, V11, V12, V13, V14, V15;
    c0, c1, c2, c3, c4, c5, c6, c7, c8, c9, c10, c11, c12, c13, c14, c15;
    C0, C1, C2, C3, C4, C5, C6, C7, C8, C9, C10, C11, C12, C13, C14, C15
);

/// Encoder for `future<T>`
pub struct FutureEncoder<W> {
    deferred: Option<DeferredFn<W>>,
}

impl<W> Default for FutureEncoder<W> {
    fn default() -> Self {
        Self { deferred: None }
    }
}

impl<W> Deferred<W> for FutureEncoder<W> {
    fn take_deferred(&mut self) -> Option<DeferredFn<W>> {
        self.deferred.take()
    }
}

impl<T, W, Fut> tokio_util::codec::Encoder<Fut> for FutureEncoder<W>
where
    T: Encode<W>,
    W: Sink,
    Fut: Future<Output = T> + Send + Unpin + 'static,
    std::io::Error: From<<T::Encoder as tokio_util::codec::Encoder<T>>::Error>,
{
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self, item), fields(ty = "future"))]
    fn encode(&mut self, mut item: Fut, dst: &mut BytesMut) -> std::io::Result<()> {
        self.deferred = None;
        if let Some(item) = (&mut item).now_or_never() {
            dst.reserve(1);
            dst.put_u8(0x01);
            self.deferred = item.encode(&mut T::Encoder::default(), dst)?;
            return Ok(());
        }
        let offset = dst.len();
        dst.reserve(1);
        dst.put_u8(0x00);
        let span = Span::current();
        self.deferred = Some(Box::new(move |w| {
            Box::pin(async move { w.write_future(offset, item).await }.instrument(span))
        }));
        Ok(())
    }
}

impl<T, W: Sink> Encode<W> for Pin<Box<dyn Future<Output = T> + Send>>
where
    T: Encode<W> + 'static,
    std::io::Error: From<<T::Encoder as tokio_util::codec::Encoder<T>>::Error>,
{
    type Encoder = FutureEncoder<W>;
}

/// Decoder for `future<T>`
pub struct FutureDecoder<T, R>
where
    R: Source,
    T: Decode<R>,
{
    dec: OptionDecoder<T::Decoder>,
    deferred: Option<DeferredFn<R>>,
    _ty: PhantomData<T>,
}

impl<T, R: Source> Default for FutureDecoder<T, R>
where
    T: Decode<R>,
{
    fn default() -> Self {
        Self {
            dec: OptionDecoder::default(),
            deferred: None,
            _ty: PhantomData,
        }
    }
}

impl<T, R: Source> Deferred<R> for FutureDecoder<T, R>
where
    T: Decode<R>,
{
    fn take_deferred(&mut self) -> Option<DeferredFn<R>> {
        self.deferred.take()
    }
}

impl<T, R: Source> tokio_util::codec::Decoder for FutureDecoder<T, R>
where
    T: Decode<R> + Send + 'static,
    std::io::Error: From<<T::Decoder as tokio_util::codec::Decoder>::Error>,
{
    type Item = Pin<Box<dyn Future<Output = T> + Send>>;
    type Error = <T::Decoder as tokio_util::codec::Decoder>::Error;

    #[instrument(level = "trace", skip(self), fields(ty = "future"))]
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let Some(item) = self.dec.decode(src)? else {
            return Ok(None);
        };
        if let Some(item) = item {
            self.deferred = self.dec.take_deferred();
            return Ok(Some(Box::pin(async { item })));
        }

        // future is pending
        let (tx, rx) = oneshot::channel();
        let dec = mem::take(&mut self.dec).into_inner();
        let span = Span::current();
        self.deferred = Some(Box::new(|r| {
            Box::pin(
                async move {
                    let mut dec = FramedRead::new(r, dec);
                    trace!("receiving future element");
                    let Some(item) = dec.next().await else {
                        return Err(std::io::ErrorKind::UnexpectedEof.into());
                    };
                    let item = item?;
                    let rx = dec.decoder_mut().take_deferred();
                    let FramedParts {
                        io: mut r,
                        read_buf,
                        ..
                    } = dec.into_parts();
                    r.unread(read_buf);
                    if tx.send(item).is_err() {
                        return Err(error_receiver_closed());
                    }
                    if let Some(rx) = rx {
                        return rx(r).await;
                    }
                    let n = r.read(&mut [0]).await?;
                    if n > 0 {
                        return Err(error_trailing_data());
                    }
                    Ok(())
                }
                .instrument(span),
            )
        }));
        Ok(Some(Box::pin(async {
            let Ok(ret) = rx.await else {
                error!("future I/O dropped");
                return pending().await;
            };
            ret
        })))
    }
}

impl<T, R: Source> Decode<R> for Pin<Box<dyn Future<Output = T> + Send>>
where
    T: Decode<R> + Send + 'static,
    std::io::Error: From<<T::Decoder as tokio_util::codec::Decoder>::Error>,
{
    type Decoder = FutureDecoder<T, R>;
    type ListDecoder = ListDecoder<Self::Decoder, R>;
}

/// Encoder for `stream<T>`
pub struct StreamEncoder<W> {
    deferred: Option<DeferredFn<W>>,
}

impl<W> Default for StreamEncoder<W> {
    fn default() -> Self {
        Self { deferred: None }
    }
}

impl<W> Deferred<W> for StreamEncoder<W> {
    fn take_deferred(&mut self) -> Option<DeferredFn<W>> {
        self.deferred.take()
    }
}

impl<T, W, S> tokio_util::codec::Encoder<S> for StreamEncoder<W>
where
    T: Encode<W> + Send + 'static,
    W: Sink,
    S: Stream<Item = Vec<T>> + Send + Unpin + 'static,
    std::io::Error: From<<T::Encoder as tokio_util::codec::Encoder<T>>::Error>,
{
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self, items), fields(ty = "stream"))]
    fn encode(&mut self, mut items: S, dst: &mut BytesMut) -> std::io::Result<()> {
        self.deferred = None;
        let mut ready = Vec::default();
        loop {
            match items.next().now_or_never() {
                Some(Some(chunk)) if ready.is_empty() => ready = chunk,
                Some(Some(chunk)) => ready.extend(chunk),
                Some(None) => {
                    dst.reserve(1);
                    dst.put_u8(0x01);
                    self.deferred = ready.encode(&mut ListEncoder::default(), dst)?;
                    return Ok(());
                }
                None => break,
            }
        }
        let items = stream::iter([ready]).chain(items);
        let offset = dst.len();
        dst.reserve(1);
        dst.put_u8(0x00);
        let span = Span::current();
        self.deferred = Some(Box::new(move |w| {
            Box::pin(async move { w.write_stream(offset, items).await }.instrument(span))
        }));
        Ok(())
    }
}

impl<T, W: Sink> Encode<W> for Pin<Box<dyn Stream<Item = Vec<T>> + Send>>
where
    T: Encode<W> + Send + 'static,
    std::io::Error: From<<T::Encoder as tokio_util::codec::Encoder<T>>::Error>,
{
    type Encoder = StreamEncoder<W>;
}

/// Encoder for `stream<list<u8>>`
pub struct StreamEncoderBytes<W> {
    deferred: Option<DeferredFn<W>>,
}

impl<W> Default for StreamEncoderBytes<W> {
    fn default() -> Self {
        Self { deferred: None }
    }
}

impl<W> Deferred<W> for StreamEncoderBytes<W> {
    fn take_deferred(&mut self) -> Option<DeferredFn<W>> {
        self.deferred.take()
    }
}

impl<W, S> tokio_util::codec::Encoder<S> for StreamEncoderBytes<W>
where
    W: Sink,
    S: Stream<Item = Bytes> + Send + Unpin + 'static,
{
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self, items), fields(ty = "stream<u8>"))]
    fn encode(&mut self, mut items: S, dst: &mut BytesMut) -> std::io::Result<()> {
        self.deferred = None;
        let mut ready = Vec::default();
        loop {
            match items.next().now_or_never() {
                Some(Some(chunk)) => ready.push(chunk),
                Some(None) => return encode_ready_bytes(&ready, dst),
                None => break,
            }
        }
        let items = stream::iter(ready).chain(items);
        let offset = dst.len();
        dst.reserve(1);
        dst.put_u8(0x00);
        self.deferred = Some(Box::new(move |w| {
            Box::pin(async move { w.write_stream_bytes(offset, items).await })
        }));
        Ok(())
    }
}

impl<W: Sink> Encode<W> for Pin<Box<dyn Stream<Item = Bytes> + Send>> {
    type Encoder = StreamEncoderBytes<W>;
}

/// Encoder for `stream<list<u8>>` with [`AsyncRead`] support
///
/// At most `MAX_READY` bytes are read from the stream on encode, any remaining
/// bytes are transmitted asynchronously.
pub struct StreamEncoderRead<W, const MAX_READY: usize = { usize::MAX }> {
    deferred: Option<DeferredFn<W>>,
}

impl<W, const MAX_READY: usize> Default for StreamEncoderRead<W, MAX_READY> {
    fn default() -> Self {
        Self { deferred: None }
    }
}

impl<W, const MAX_READY: usize> Deferred<W> for StreamEncoderRead<W, MAX_READY> {
    fn take_deferred(&mut self) -> Option<DeferredFn<W>> {
        self.deferred.take()
    }
}

impl<W, S, const MAX_READY: usize> tokio_util::codec::Encoder<S> for StreamEncoderRead<W, MAX_READY>
where
    W: Sink,
    S: AsyncRead + Send + Unpin + 'static,
{
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self, items), fields(ty = "stream<u8>"))]
    fn encode(&mut self, mut items: S, dst: &mut BytesMut) -> std::io::Result<()> {
        self.deferred = None;
        let mut ready = BytesMut::default();
        loop {
            let n = MAX_READY.saturating_sub(ready.len());
            if n == 0 {
                break;
            }
            ready.reserve(n.min(READ_CHUNK_SIZE));
            match items.read_buf(&mut (&mut ready).limit(n)).now_or_never() {
                Some(Ok(0)) => return encode_ready_bytes(&[ready], dst),
                Some(Ok(..)) => {}
                Some(Err(err)) => return Err(err),
                None => break,
            }
        }
        let offset = dst.len();
        dst.reserve(1);
        dst.put_u8(0x00);
        self.deferred = Some(Box::new(move |w| {
            Box::pin(async move { w.write_stream_read(offset, ready, items).await })
        }));
        Ok(())
    }
}

impl<W: Sink> Encode<W> for Pin<Box<dyn AsyncRead + Send>> {
    type Encoder = StreamEncoderRead<W>;
}

impl<T, W: Sink> Encode<W> for std::io::Cursor<T>
where
    T: AsRef<[u8]> + Send + Unpin + 'static,
{
    type Encoder = StreamEncoderRead<W>;
}

impl<W: Sink> Encode<W> for tokio::io::Empty {
    type Encoder = StreamEncoderRead<W>;
}

#[cfg(feature = "io-std")]
impl<W: Sink> Encode<W> for tokio::io::Stdin {
    type Encoder = StreamEncoderRead<W>;
}

#[cfg(feature = "fs")]
impl<W: Sink> Encode<W> for tokio::fs::File {
    type Encoder = StreamEncoderRead<W>;
}

#[cfg(feature = "net")]
impl<W: Sink> Encode<W> for tokio::net::TcpStream {
    type Encoder = StreamEncoderRead<W>;
}

#[cfg(all(unix, feature = "net"))]
impl<W: Sink> Encode<W> for tokio::net::UnixStream {
    type Encoder = StreamEncoderRead<W>;
}

#[cfg(all(unix, feature = "net"))]
impl<W: Sink> Encode<W> for tokio::net::unix::pipe::Receiver {
    type Encoder = StreamEncoderRead<W>;
}

/// Decoder for `stream<T>`
pub struct StreamDecoder<T, R>
where
    R: Source,
    T: Decode<R>,
{
    dec: OptionDecoder<T::ListDecoder>,
    deferred: Option<DeferredFn<R>>,
    _ty: PhantomData<T>,
}

impl<T, R: Source> Default for StreamDecoder<T, R>
where
    T: Decode<R>,
{
    fn default() -> Self {
        Self {
            dec: OptionDecoder::default(),
            deferred: None,
            _ty: PhantomData,
        }
    }
}

impl<T, R: Source> Deferred<R> for StreamDecoder<T, R>
where
    T: Decode<R>,
{
    fn take_deferred(&mut self) -> Option<DeferredFn<R>> {
        self.deferred.take()
    }
}

fn receive_pending_stream<T: Send + 'static>(
    rx: mpsc::Receiver<T>,
    end: oneshot::Receiver<()>,
) -> impl Stream<Item = T> + Send {
    stream::unfold((rx, end), |(mut rx, end)| async move {
        if let Some(item) = rx.recv().await {
            return Some((item, (rx, end)));
        }
        if end.await.is_err() {
            error!("stream I/O dropped");
            return pending().await;
        }
        None
    })
}

#[instrument(level = "trace", skip(dec, r, tx, end_tx), ret)]
async fn handle_deferred_stream<C, T, R: Source>(
    dec: C,
    r: R,
    tx: mpsc::Sender<Vec<T>>,
    end_tx: oneshot::Sender<()>,
) -> std::io::Result<()>
where
    C: tokio_util::codec::Decoder<Item = T> + Deferred<R>,
    std::io::Error: From<C::Error>,
{
    let dec = ListDecoder::new(dec);
    let mut framed = FramedRead::new(r, dec);
    let mut tasks = FuturesUnordered::default();
    let mut pending = Option::default();
    let mut i = 0_usize;
    loop {
        trace!("receiving pending stream chunk");
        select! {
            chunk = framed.next(), if pending.is_none() => {
                let Some(chunk) = chunk else {
                    break;
                };
                let chunk = chunk?;
                let end = i.checked_add(chunk.len()).ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "stream element index would overflow usize",
                    )
                })?;
                trace!(i, end, "received stream chunk");
                for (i, deferred) in zip(i.., mem::take(&mut framed.decoder_mut().deferred)) {
                    if let Some(deferred) = deferred {
                        let r = framed.get_ref().index(&[i]).map_err(std::io::Error::other)?;
                        tasks.push(deferred(r));
                    }
                }
                pending = Some(chunk);
                i = end;
            },
            permit = tx.reserve(), if pending.is_some() => {
                let Ok(permit) = permit else {
                    return Err(error_receiver_closed());
                };
                if let Some(chunk) = pending.take() {
                    permit.send(chunk);
                }
            },
            Some(res) = tasks.next() => {
                trace!(?res, "receiver task finished");
                res?;
            }
        }
    }
    trace!("received stream end");
    _ = end_tx.send(());
    drop(tx);
    while let Some(res) = tasks.next().await {
        res?;
    }
    Ok(())
}

impl<T, R: Source> tokio_util::codec::Decoder for StreamDecoder<T, R>
where
    T: Decode<R> + Send + 'static,
    T::ListDecoder: Deferred<R>,
    <T::Decoder as tokio_util::codec::Decoder>::Error: Send,
    std::io::Error: From<<T::Decoder as tokio_util::codec::Decoder>::Error>,
{
    type Item = Pin<Box<dyn Stream<Item = Vec<T>> + Send>>;
    type Error = <<T as Decode<R>>::ListDecoder as tokio_util::codec::Decoder>::Error;

    #[instrument(level = "trace", skip(self), fields(ty = "stream"))]
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let Some(chunk) = self.dec.decode(src)? else {
            return Ok(None);
        };
        if let Some(chunk) = chunk {
            self.deferred = self.dec.take_deferred();
            if chunk.is_empty() {
                return Ok(Some(Box::pin(stream::empty())));
            }
            return Ok(Some(Box::pin(stream::iter([chunk]))));
        }

        // stream is pending
        let (tx, rx) = mpsc::channel(128);
        let (end_tx, end_rx) = oneshot::channel();
        self.deferred = Some(Box::new(|r| {
            Box::pin(
                async move { handle_deferred_stream(T::Decoder::default(), r, tx, end_tx).await },
            )
        }));
        Ok(Some(Box::pin(receive_pending_stream(rx, end_rx))))
    }
}

impl<T, R: Source> Decode<R> for Pin<Box<dyn Stream<Item = Vec<T>> + Send>>
where
    T: Decode<R> + Send + 'static,
    T::ListDecoder: Deferred<R> + Send,
    <T::Decoder as tokio_util::codec::Decoder>::Error: Send,
    std::io::Error: From<<T::Decoder as tokio_util::codec::Decoder>::Error>,
{
    type Decoder = StreamDecoder<T, R>;
    type ListDecoder = ListDecoder<Self::Decoder, R>;
}

/// Decoder for `stream<list<u8>>`
pub struct StreamDecoderBytes<R> {
    dec: OptionDecoder<CoreVecDecoderBytes>,
    deferred: Option<DeferredFn<R>>,
}

impl<R> Default for StreamDecoderBytes<R> {
    fn default() -> Self {
        Self {
            dec: OptionDecoder::default(),
            deferred: None,
        }
    }
}

impl<R: Source> Deferred<R> for StreamDecoderBytes<R> {
    fn take_deferred(&mut self) -> Option<DeferredFn<R>> {
        self.deferred.take()
    }
}

impl<R: Source> tokio_util::codec::Decoder for StreamDecoderBytes<R> {
    type Item = Pin<Box<dyn Stream<Item = Bytes> + Send>>;
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self), fields(ty = "stream<u8>"))]
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let Some(chunk) = self.dec.decode(src)? else {
            return Ok(None);
        };
        if let Some(chunk) = chunk {
            if chunk.is_empty() {
                return Ok(Some(Box::pin(stream::empty())));
            }
            return Ok(Some(Box::pin(stream::iter([chunk]))));
        }

        // stream is pending
        let (tx, rx) = mpsc::channel(128);
        let (end_tx, end_rx) = oneshot::channel();
        let dec = mem::take(&mut self.dec).into_inner();
        let span = Span::current();
        self.deferred = Some(Box::new(|r| {
            Box::pin(
                async move {
                    let mut framed = FramedRead::new(r, dec);
                    trace!("receiving pending byte stream chunk");
                    while let Some(chunk) = framed.next().await {
                        let chunk = chunk?;
                        trace!(?chunk, "received pending byte stream chunk");
                        if tx.send(chunk).await.is_err() {
                            return Err(error_receiver_closed());
                        }
                    }
                    trace!("received stream end");
                    _ = end_tx.send(());
                    Ok(())
                }
                .instrument(span),
            )
        }));
        Ok(Some(Box::pin(receive_pending_stream(rx, end_rx))))
    }
}

impl<R: Source> Decode<R> for Pin<Box<dyn Stream<Item = Bytes> + Send>> {
    type Decoder = StreamDecoderBytes<R>;
    type ListDecoder = ListDecoder<Self::Decoder, R>;
}

/// Decoder for `stream<list<u8>>` with [`AsyncRead`] support
pub struct StreamDecoderRead<R> {
    dec: StreamDecoderBytes<R>,
}

impl<R> Default for StreamDecoderRead<R> {
    fn default() -> Self {
        Self {
            dec: StreamDecoderBytes::default(),
        }
    }
}

impl<R: Source> Deferred<R> for StreamDecoderRead<R> {
    fn take_deferred(&mut self) -> Option<DeferredFn<R>> {
        self.dec.take_deferred()
    }
}

impl<R: Source> tokio_util::codec::Decoder for StreamDecoderRead<R> {
    type Item = Pin<Box<dyn AsyncRead + Send>>;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let Some(s) = self.dec.decode(src)? else {
            return Ok(None);
        };
        Ok(Some(Box::pin(StreamReader::new(
            s.map(Ok::<_, std::io::Error>).fuse(),
        ))))
    }
}

impl<R: Source> Decode<R> for Pin<Box<dyn AsyncRead + Send>> {
    type Decoder = StreamDecoderRead<R>;
    type ListDecoder = ListDecoder<Self::Decoder, R>;
}

#[cfg(test)]
mod tests {
    use core::future::ready;

    use std::io::ErrorKind;

    use anyhow::bail;
    use tokio::io::{BufReader, duplex, sink};
    use tokio::task::yield_now;
    use tokio_util::codec::Decoder as _;

    use super::*;
    use crate::frame::conn::mux;

    const CLOSED: &[u8] = b"\0\0";

    type Reader = Incoming<&'static [u8]>;
    type Writer = Outgoing<tokio::io::Sink>;

    type StreamBytes = Pin<Box<dyn Stream<Item = Bytes> + Send>>;

    #[test_log::test(tokio::test)]
    async fn future_closes_after_nested_value() -> anyhow::Result<()> {
        let (tx, rx) = duplex(1024);
        let (mut w, _rx) = mux(tx, CLOSED, [[None; 0]; 0]);
        let (.., incoming) = mux(sink(), BufReader::new(rx), [[Some(0)]]);
        let mut r = incoming.index(&[0])?;
        let nested: StreamBytes = Box::pin(stream::once(async {
            yield_now().await;
            Bytes::from_static(b"x")
        }));
        w.index(&[0])?.write_future(0, ready(nested)).await?;
        w.shutdown().await?;
        drop(w);
        let mut buf = Vec::default();
        r.read_to_end(&mut buf).await?;
        assert_eq!(buf, b"\x00\x01x");
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn codec() -> anyhow::Result<()> {
        let mut buf = BytesMut::default();
        let mut enc = <(u8, u32) as Encode<Writer>>::Encoder::default();
        enc.encode((0x42, 0x42), &mut buf)?;
        if let Some(_f) = Deferred::<Writer>::take_deferred(&mut enc) {
            bail!("no deferred write should have been returned");
        }
        assert_eq!(buf.as_ref(), b"\x42\x42");
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn array_codec() -> anyhow::Result<()> {
        let mut buf = BytesMut::default();
        let mut enc = <[[u32; 2]; 2] as Encode<Writer>>::Encoder::default();
        enc.encode([[1, 300], [2, 3]], &mut buf)?;
        assert_eq!(buf.as_ref(), b"\x01\xac\x02\x02\x03");

        let mut dec = <[[u32; 2]; 2] as Decode<Reader>>::Decoder::default();
        let v = dec.decode(&mut buf.split_to(3))?;
        assert_eq!(v, None);
        let v = dec.decode(&mut buf)?;
        assert_eq!(v, Some([[1, 300], [2, 3]]));
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn list_decoder() -> anyhow::Result<()> {
        let mut dec = <Vec<Vec<String>> as Decode<Reader>>::Decoder::default();
        let mut buf = BytesMut::from(&b"\x02\x01\x01a\x01\x01\xff"[..]);
        assert!(dec.decode(&mut buf).is_err());
        let mut buf = BytesMut::from("\x01\x01\x01b");
        let v = dec.decode(&mut buf)?;
        assert_eq!(v, Some(vec![vec!["b".to_string()]]));

        let mut dec = <Vec<StreamBytes> as Decode<Reader>>::Decoder::default();
        let mut buf = BytesMut::from(&b"\x02\0\xff\xff\xff\xff\xff"[..]);
        assert!(dec.decode(&mut buf).is_err());
        let vs = dec.decode(&mut BytesMut::from("\0"))?;
        assert!(vs.is_some_and(|vs| vs.is_empty()));
        assert!(Deferred::<Reader>::take_deferred(&mut dec).is_none());

        let n = b"\xff\xff\xff\xff\x0f";
        let err = crate::unpack::<Vec<Vec<String>>>(&mut BytesMut::from(&n[..])).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::UnexpectedEof);
        let err = crate::unpack::<Vec<StreamBytes>>(&mut BytesMut::from(&n[..]))
            .err()
            .unwrap();
        assert_eq!(err.kind(), ErrorKind::UnexpectedEof);
        let err = crate::unpack::<Bytes>(&mut BytesMut::from(&n[..])).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::UnexpectedEof);
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn stream_encoder() -> anyhow::Result<()> {
        async fn read_frames(mut rx: impl AsyncRead + Unpin) -> anyhow::Result<Vec<u8>> {
            let mut buf = vec![];
            rx.read_to_end(&mut buf).await?;
            let mut buf = buf.as_slice();
            let mut data = vec![];
            while let [0x01, 0x00, n, rest @ ..] = buf {
                let (chunk, rest) = rest.split_at(usize::from(*n));
                data.extend_from_slice(chunk);
                buf = rest;
            }
            if !buf.is_empty() {
                bail!("unexpected trailing bytes: {buf:02x?}");
            }
            Ok(data)
        }

        for (ready, expected) in [("foo", &b"\x03foo\x03bar"[..]), ("", b"\x03bar")] {
            let (tx, rx) = duplex(1024);
            let (root, _rx) = mux(tx, CLOSED, [[None; 0]; 0]);
            let w = root.index(&[0])?;
            drop(root);
            w.write_stream_bytes(0, stream::iter([ready, "bar"].map(Bytes::from)))
                .await?;
            assert_eq!(read_frames(rx).await?, expected);

            let (tx, rx) = duplex(1024);
            let (root, _rx) = mux(tx, CLOSED, [[None; 0]; 0]);
            let w = root.index(&[0])?;
            drop(root);
            w.write_stream_read(0, ready.into(), b"bar".as_slice())
                .await?;
            assert_eq!(read_frames(rx).await?, expected);
        }
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn deferred_decoder() -> anyhow::Result<()> {
        type FutureBytes = Pin<Box<dyn Future<Output = Bytes> + Send>>;
        type UnitFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

        fn buffered(buffer: &[u8]) -> Reader {
            let (.., mut rx) = mux(sink(), CLOSED, Vec::<Vec<Option<usize>>>::default());
            rx.unread(BytesMut::from(buffer));
            rx
        }

        fn pending<T: Decode<Reader>>() -> (T, DeferredFn<Reader>) {
            let mut dec = T::Decoder::default();
            let Ok(Some(v)) = dec.decode(&mut BytesMut::from("\0")) else {
                panic!("value not decoded")
            };
            (v, dec.take_deferred().expect("deferred read missing"))
        }

        async fn read<T: Decode<Reader>>(buffer: &[u8]) -> (T, std::io::Result<()>) {
            let (v, f) = pending::<T>();
            let res = f(buffered(buffer)).await;
            (v, res)
        }

        let (fut, res) = read::<UnitFuture>(b"").await;
        res.unwrap();
        fut.await;

        let (fut, res) = read::<UnitFuture>(b"\x2a").await;
        assert_eq!(res.unwrap_err().kind(), ErrorKind::InvalidData);
        fut.await;

        let data: Bytes = (0..2 * READ_CHUNK_SIZE).map(|i| i as u8).collect();
        let mut buffer = BytesMut::default();
        CoreVecEncoderBytes.encode(&data, &mut buffer).unwrap();
        buffer.extend_from_slice(&b"\x2a".repeat(READ_CHUNK_SIZE));
        let (.., res) = read::<FutureBytes>(&buffer).await;
        assert_eq!(res.unwrap_err().kind(), ErrorKind::InvalidData);

        let (.., res) = read::<FutureBytes>(b"\x03foo\x2a").await;
        assert_eq!(res.unwrap_err().kind(), ErrorKind::InvalidData);

        let (.., res) = read::<StreamBytes>(b"\x03fo").await;
        assert_eq!(res.unwrap_err().kind(), ErrorKind::UnexpectedEof);

        let (fut, f) = pending::<FutureBytes>();
        drop(fut);
        let err = f(buffered(b"\x03foo")).await;
        assert_eq!(err.unwrap_err().kind(), ErrorKind::BrokenPipe);

        let (mut s, res) = read::<StreamBytes>(b"\x03foo").await;
        res?;
        assert_eq!(s.next().await.unwrap(), "foo");
        assert!(s.next().await.is_none());

        let (mut s, res) =
            read::<Pin<Box<dyn Stream<Item = Vec<String>> + Send>>>(b"\x01\x03foo").await;
        res?;
        assert_eq!(s.next().await.unwrap(), ["foo"]);
        assert!(s.next().await.is_none());

        let (mut r, res) = read::<Pin<Box<dyn AsyncRead + Send>>>(b"\x03foo").await;
        res?;
        let mut buf = [0; 3];
        r.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"foo");
        assert_eq!(r.read(&mut buf).await?, 0);

        let data: Bytes = (0..2 * READ_CHUNK_SIZE).map(|i| i as u8).collect();
        let mut buffer = BytesMut::from("\0");
        CoreVecEncoderBytes.encode(&data, &mut buffer)?;
        let (fut, res) = read::<Pin<Box<dyn Future<Output = FutureBytes> + Send>>>(&buffer).await;
        res?;
        assert_eq!(fut.await.await, data);
        Ok(())
    }
}
