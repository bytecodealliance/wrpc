use core::any::TypeId;
use core::fmt::{self, Debug};
use core::future::{Future, pending};
use core::hash::{Hash, Hasher};
use core::iter::zip;
use core::marker::PhantomData;
use core::mem;
use core::ops::{Deref, DerefMut};
use core::pin::Pin;

use bytes::{Buf as _, BufMut as _, Bytes, BytesMut};
use futures::stream::{self, FuturesUnordered};
use futures::{Stream, StreamExt as _, TryStreamExt as _};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::select;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::codec::{Encoder as _, FramedRead};
use tokio_util::io::StreamReader;
use tracing::{Instrument as _, Span, debug, error, instrument, trace};
use wasm_tokio::cm::{
    BoolCodec, F32Codec, F64Codec, OptionDecoder, OptionEncoder, PrimValEncoder, ResultDecoder,
    ResultEncoder, S8Codec, S16Codec, S32Codec, S64Codec, TupleDecoder, TupleEncoder, U8Codec,
    U16Codec, U32Codec, U64Codec,
};
use wasm_tokio::{
    CoreNameDecoder, CoreNameEncoder, CoreVecDecoder, CoreVecDecoderBytes, CoreVecEncoderBytes,
    Leb128DecoderI8, Leb128DecoderI16, Leb128DecoderI32, Leb128DecoderI64, Leb128DecoderI128,
    Leb128DecoderU8, Leb128DecoderU16, Leb128DecoderU32, Leb128DecoderU64, Leb128DecoderU128,
    Leb128Encoder, Utf8Codec,
};

use crate::Incoming;
use crate::frame::Outgoing;

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
pub type DeferredFn<T> = Box<
    dyn FnOnce(T, Vec<usize>) -> Pin<Box<dyn Future<Output = std::io::Result<()>> + Send>> + Send,
>;

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

macro_rules! impl_handle_deferred {
    ($name:ident, $t:ty) => {
        #[instrument(level = "trace", skip(w, deferred))]
        async fn $name<I>(w: $t, deferred: I, mut path: Vec<usize>) -> std::io::Result<()>
        where
            I: IntoIterator<Item = Option<DeferredFn<$t>>>,
            I::IntoIter: ExactSizeIterator,
        {
            let mut futs = FuturesUnordered::default();
            for (i, f) in zip(0.., deferred) {
                if let Some(f) = f {
                    path.push(i);
                    let w = w.index(&path).map_err(std::io::Error::other)?;
                    path.pop();
                    futs.push(f(w, Vec::default()));
                }
            }
            while let Some(()) = futs.try_next().await? {}
            Ok(())
        }
    };
}

impl_handle_deferred!(handle_deferred_tx, Outgoing);
impl_handle_deferred!(handle_deferred_rx, Incoming);

/// Defines value encoding
pub trait Encode: Sized {
    /// Encoder used to encode the value
    type Encoder: tokio_util::codec::Encoder<Self> + Deferred<Outgoing> + Default + Send;

    /// Convenience function for encoding a value
    #[instrument(level = "trace", skip(self, enc))]
    fn encode(
        self,
        enc: &mut Self::Encoder,
        dst: &mut BytesMut,
    ) -> Result<
        Option<DeferredFn<Outgoing>>,
        <Self::Encoder as tokio_util::codec::Encoder<Self>>::Error,
    > {
        enc.encode(self, dst)?;
        Ok(enc.take_deferred())
    }

    /// Encode an iterator of owned values
    #[instrument(level = "trace", skip(items, enc))]
    fn encode_iter_own<I>(
        items: I,
        enc: &mut Self::Encoder,
        dst: &mut BytesMut,
    ) -> Result<
        Option<DeferredFn<Outgoing>>,
        <Self::Encoder as tokio_util::codec::Encoder<Self>>::Error,
    >
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
            Ok(Some(Box::new(move |w, path| {
                Box::pin(handle_deferred_tx(w, deferred, path))
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
    ) -> Result<
        Option<DeferredFn<Outgoing>>,
        <Self::Encoder as tokio_util::codec::Encoder<&'a Self>>::Error,
    >
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
            Ok(Some(Box::new(move |w, path| {
                Box::pin(handle_deferred_tx(w, deferred, path))
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
    ) -> Result<
        Option<DeferredFn<Outgoing>>,
        <Self::Encoder as tokio_util::codec::Encoder<Self>>::Error,
    > {
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
    ) -> Result<
        Option<DeferredFn<Outgoing>>,
        <Self::Encoder as tokio_util::codec::Encoder<&'a Self>>::Error,
    >
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
pub trait Decode: Sized {
    /// Decoder used to decode value
    type Decoder: tokio_util::codec::Decoder<Item = Self>
        + Deferred<Incoming>
        + Default
        + Send
        + 'static;
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

impl<T> Encode for Option<T>
where
    T: Encode,
{
    type Encoder = OptionEncoder<T::Encoder>;
}

impl<'a, T> Encode for &'a Option<T>
where
    T: Encode,
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

impl<T> Decode for Option<T>
where
    T: Decode,
{
    type Decoder = OptionDecoder<T::Decoder>;
    type ListDecoder = ListDecoder<Self::Decoder>;
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
                    panic!("both `result::ok` and `result::err` deferred function set")
                }
                Some(ok)
            }
        }
    }
}

impl<O, E> Encode for Result<O, E>
where
    O: Encode,
    E: Encode,
    std::io::Error: From<<O::Encoder as tokio_util::codec::Encoder<O>>::Error>,
    std::io::Error: From<<E::Encoder as tokio_util::codec::Encoder<E>>::Error>,
{
    type Encoder = ResultEncoder<O::Encoder, E::Encoder>;
}

impl<'a, O, E> Encode for &'a Result<O, E>
where
    O: Encode,
    O::Encoder: tokio_util::codec::Encoder<&'a O>,
    E: Encode,
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
                    panic!("both `result::ok` and `result::err` deferred function set")
                }
                Some(ok)
            }
        }
    }
}

impl<O, E> Decode for Result<O, E>
where
    O: Decode,
    E: Decode,
    std::io::Error: From<<O::Decoder as tokio_util::codec::Decoder>::Error>,
    std::io::Error: From<<E::Decoder as tokio_util::codec::Decoder>::Error>,
{
    type Decoder = ResultDecoder<O::Decoder, E::Decoder>;
    type ListDecoder = ListDecoder<Self::Decoder>;
}

/// Encoder for `list<T>`
#[derive(Default)]
pub struct ListEncoder {
    deferred: Option<DeferredFn<Outgoing>>,
}

impl Deferred<Outgoing> for ListEncoder {
    fn take_deferred(&mut self) -> Option<DeferredFn<Outgoing>> {
        self.deferred.take()
    }
}

impl<T> tokio_util::codec::Encoder<Vec<T>> for ListEncoder
where
    T: Encode,
{
    type Error = <T::Encoder as tokio_util::codec::Encoder<T>>::Error;

    fn encode(&mut self, items: Vec<T>, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let mut enc = T::Encoder::default();
        self.deferred = T::encode_list_own(items, &mut enc, dst)?;
        Ok(())
    }
}

impl<'a, T> tokio_util::codec::Encoder<&'a Vec<T>> for ListEncoder
where
    T: Encode,
    T::Encoder: tokio_util::codec::Encoder<&'a T>,
{
    type Error = <T::Encoder as tokio_util::codec::Encoder<&'a T>>::Error;

    fn encode(&mut self, items: &'a Vec<T>, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let mut enc = T::Encoder::default();
        self.deferred = T::encode_list_ref(items, &mut enc, dst)?;
        Ok(())
    }
}

impl<'a, 'b, T> tokio_util::codec::Encoder<&'a &'b Vec<T>> for ListEncoder
where
    T: Encode,
    T::Encoder: tokio_util::codec::Encoder<&'b T>,
{
    type Error = <T::Encoder as tokio_util::codec::Encoder<&'b T>>::Error;

    fn encode(&mut self, items: &'a &'b Vec<T>, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let mut enc = T::Encoder::default();
        self.deferred = T::encode_list_ref(items, &mut enc, dst)?;
        Ok(())
    }
}

impl<'a, T> tokio_util::codec::Encoder<&'a [T]> for ListEncoder
where
    T: Encode,
    T::Encoder: tokio_util::codec::Encoder<&'a T>,
{
    type Error = <T::Encoder as tokio_util::codec::Encoder<&'a T>>::Error;

    fn encode(&mut self, items: &'a [T], dst: &mut BytesMut) -> Result<(), Self::Error> {
        let mut enc = T::Encoder::default();
        self.deferred = T::encode_list_ref(items, &mut enc, dst)?;
        Ok(())
    }
}

impl<'a, 'b, T> tokio_util::codec::Encoder<&'a &'b [T]> for ListEncoder
where
    T: Encode,
    T::Encoder: tokio_util::codec::Encoder<&'b T>,
{
    type Error = <T::Encoder as tokio_util::codec::Encoder<&'b T>>::Error;

    fn encode(&mut self, items: &'a &'b [T], dst: &mut BytesMut) -> Result<(), Self::Error> {
        let mut enc = T::Encoder::default();
        self.deferred = T::encode_list_ref(items, &mut enc, dst)?;
        Ok(())
    }
}

impl<T> Encode for Vec<T>
where
    T: Encode,
{
    type Encoder = ListEncoder;
}

impl<'a, T> Encode for &'a Vec<T>
where
    T: Encode,
    T::Encoder: tokio_util::codec::Encoder<&'a T>,
{
    type Encoder = ListEncoder;
}

impl<'a, T> Encode for &'a [T]
where
    T: Encode,
    T::Encoder: tokio_util::codec::Encoder<&'a T>,
{
    type Encoder = ListEncoder;
}

/// Decoder for `list<T>`
pub struct ListDecoder<T>
where
    T: tokio_util::codec::Decoder,
{
    dec: T,
    ret: Vec<T::Item>,
    cap: usize,
    deferred: Vec<Option<DeferredFn<Incoming>>>,
}

impl<T> ListDecoder<T>
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

impl<T> Default for ListDecoder<T>
where
    T: tokio_util::codec::Decoder + Default,
{
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T> Deferred<Incoming> for ListDecoder<T>
where
    T: tokio_util::codec::Decoder,
{
    fn take_deferred(&mut self) -> Option<DeferredFn<Incoming>> {
        let deferred = mem::take(&mut self.deferred);
        if deferred.iter().any(Option::is_some) {
            Some(Box::new(|r, path| {
                Box::pin(handle_deferred_rx(r, deferred, path))
            }))
        } else {
            None
        }
    }
}

impl<T> tokio_util::codec::Decoder for ListDecoder<T>
where
    T: tokio_util::codec::Decoder + Deferred<Incoming>,
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
            let len = len
                .try_into()
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
            self.ret = Vec::with_capacity(len);
            self.deferred = Vec::with_capacity(len);
            self.cap = len;
        }
        while self.cap > 0 {
            let Some(v) = self.dec.decode(src)? else {
                return Ok(None);
            };
            self.ret.push(v);
            self.deferred.push(self.dec.take_deferred());
            self.cap -= 1;
        }
        Ok(Some(mem::take(&mut self.ret)))
    }
}

impl<T> Decode for Vec<T>
where
    T: Decode + Send,
    T::ListDecoder: Deferred<Incoming> + Send,
{
    type Decoder = T::ListDecoder;
    type ListDecoder = ListDecoder<Self::Decoder>;
}

macro_rules! impl_copy_codec {
    ($t:ty, $c:tt) => {
        impl Encode for $t {
            type Encoder = $c;

            #[instrument(level = "trace", skip(items))]
            fn encode_iter_own<I>(
                items: I,
                enc: &mut Self::Encoder,
                dst: &mut BytesMut,
            ) -> Result<
                Option<DeferredFn<Outgoing>>,
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
                Option<DeferredFn<Outgoing>>,
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

        impl<'b> Encode for &'b $t {
            type Encoder = $c;

            #[instrument(level = "trace", skip(items))]
            fn encode_iter_own<I>(
                items: I,
                enc: &mut Self::Encoder,
                dst: &mut BytesMut,
            ) -> Result<
                Option<DeferredFn<Outgoing>>,
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
                Option<DeferredFn<Outgoing>>,
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

        impl Decode for $t {
            type Decoder = $c;
            type ListDecoder = CoreVecDecoder<Self::Decoder>;
        }
    };
}

// The Component Model canonical ABI mandates a single canonical `NaN`
// representation for floating point values. Encoding canonicalizes `NaN`s to
// match; decoding is lenient and accepts any `NaN` representation.
//
// See `canonicalize_nan{32,64}` in
// <https://github.com/WebAssembly/component-model/blob/main/design/mvp/canonical-abi/definitions.py>.
const CANONICAL_NAN_F32: u32 = 0x7fc0_0000;
const CANONICAL_NAN_F64: u64 = 0x7ff8_0000_0000_0000;

/// Defines a floating-point codec that canonicalizes `NaN` values on encode to
/// match the Component Model canonical ABI, delegating the actual byte encoding
/// and decoding to the wrapped `wasm-tokio` codec.
macro_rules! impl_canonical_nan_codec {
    ($name:ident, $inner:ty, $t:ty, $canon:expr_2021) => {
        #[doc = concat!("Canonicalizes `NaN`s on encode, wrapping [`", stringify!($inner), "`].")]
        #[derive(Debug, Default)]
        pub struct $name($inner);

        impl tokio_util::codec::Encoder<$t> for $name {
            type Error = std::io::Error;

            fn encode(&mut self, item: $t, dst: &mut BytesMut) -> Result<(), Self::Error> {
                let item = if item.is_nan() {
                    <$t>::from_bits($canon)
                } else {
                    item
                };
                self.0.encode(item, dst)
            }
        }

        impl tokio_util::codec::Encoder<&$t> for $name {
            type Error = std::io::Error;

            fn encode(&mut self, item: &$t, dst: &mut BytesMut) -> Result<(), Self::Error> {
                tokio_util::codec::Encoder::<$t>::encode(self, *item, dst)
            }
        }

        impl tokio_util::codec::Encoder<&&$t> for $name {
            type Error = std::io::Error;

            fn encode(&mut self, item: &&$t, dst: &mut BytesMut) -> Result<(), Self::Error> {
                tokio_util::codec::Encoder::<$t>::encode(self, **item, dst)
            }
        }

        impl tokio_util::codec::Decoder for $name {
            type Item = $t;
            type Error = std::io::Error;

            fn decode(&mut self, src: &mut BytesMut) -> Result<Option<$t>, Self::Error> {
                self.0.decode(src)
            }
        }

        impl_deferred_sync!($name);
        impl_deferred_sync!(CoreVecDecoder<$name>);
    };
}

impl_canonical_nan_codec!(CanonicalNanF32Codec, F32Codec, f32, CANONICAL_NAN_F32);
impl_canonical_nan_codec!(CanonicalNanF64Codec, F64Codec, f64, CANONICAL_NAN_F64);

impl_copy_codec!(bool, BoolCodec);
impl_copy_codec!(i8, S8Codec);
impl_copy_codec!(i16, S16Codec);
impl_copy_codec!(u16, U16Codec);
impl_copy_codec!(i32, S32Codec);
impl_copy_codec!(u32, U32Codec);
impl_copy_codec!(i64, S64Codec);
impl_copy_codec!(u64, U64Codec);
impl_copy_codec!(f32, CanonicalNanF32Codec);
impl_copy_codec!(f64, CanonicalNanF64Codec);
impl_copy_codec!(char, Utf8Codec);

impl Encode for u8 {
    type Encoder = U8Codec;

    #[instrument(level = "trace", skip(items))]
    fn encode_iter_own<I>(
        items: I,
        enc: &mut Self::Encoder,
        dst: &mut BytesMut,
    ) -> Result<
        Option<DeferredFn<Outgoing>>,
        <Self::Encoder as tokio_util::codec::Encoder<Self>>::Error,
    >
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
    ) -> Result<
        Option<DeferredFn<Outgoing>>,
        <Self::Encoder as tokio_util::codec::Encoder<&'a Self>>::Error,
    >
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
    ) -> Result<
        Option<DeferredFn<Outgoing>>,
        <Self::Encoder as tokio_util::codec::Encoder<Self>>::Error,
    > {
        CoreVecEncoderBytes.encode(items, dst)?;
        Ok(None)
    }

    #[instrument(level = "trace", skip(items), fields(ty = "list<u8>"))]
    fn encode_list_ref<'a>(
        items: &'a [Self],
        enc: &mut Self::Encoder,
        dst: &mut BytesMut,
    ) -> Result<
        Option<DeferredFn<Outgoing>>,
        <Self::Encoder as tokio_util::codec::Encoder<&'a Self>>::Error,
    >
    where
        Self::Encoder: tokio_util::codec::Encoder<&'a Self>,
    {
        CoreVecEncoderBytes.encode(items, dst)?;
        Ok(None)
    }
}

impl<'b> Encode for &'b u8 {
    type Encoder = U8Codec;

    #[instrument(level = "trace", skip(items))]
    fn encode_iter_own<I>(
        items: I,
        enc: &mut Self::Encoder,
        dst: &mut BytesMut,
    ) -> Result<
        Option<DeferredFn<Outgoing>>,
        <Self::Encoder as tokio_util::codec::Encoder<Self>>::Error,
    >
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
    ) -> Result<
        Option<DeferredFn<Outgoing>>,
        <Self::Encoder as tokio_util::codec::Encoder<&'a Self>>::Error,
    >
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

impl Decode for u8 {
    type Decoder = U8Codec;
    type ListDecoder = ListDecoderU8;
}

impl Encode for &str {
    type Encoder = CoreNameEncoder;
}

impl Encode for &&str {
    type Encoder = CoreNameEncoder;
}

impl Encode for String {
    type Encoder = CoreNameEncoder;
}

impl Encode for &String {
    type Encoder = CoreNameEncoder;
}

impl Decode for String {
    type Decoder = CoreNameDecoder;
    type ListDecoder = CoreVecDecoder<Self::Decoder>;
}

impl Encode for Bytes {
    type Encoder = CoreVecEncoderBytes;
}

impl Encode for &Bytes {
    type Encoder = CoreVecEncoderBytes;
}

impl Decode for Bytes {
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

impl<T: ?Sized> Encode for ResourceOwn<T> {
    type Encoder = ResourceEncoder;
}

impl<T: ?Sized> Encode for &ResourceOwn<T> {
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

impl<T: ?Sized> Encode for ResourceBorrow<T> {
    type Encoder = ResourceEncoder;
}

impl<T: ?Sized> Encode for &ResourceBorrow<T> {
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

impl<T: ?Sized> Deferred<Incoming> for ResourceBorrowDecoder<T> {
    fn take_deferred(&mut self) -> Option<DeferredFn<Incoming>> {
        None
    }
}

impl<T: ?Sized> Deferred<Incoming> for CoreVecDecoder<ResourceBorrowDecoder<T>> {
    fn take_deferred(&mut self) -> Option<DeferredFn<Incoming>> {
        None
    }
}

impl<T: ?Sized + Send + 'static> Decode for ResourceBorrow<T> {
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

impl<T: ?Sized> Deferred<Incoming> for ResourceOwnDecoder<T> {
    fn take_deferred(&mut self) -> Option<DeferredFn<Incoming>> {
        None
    }
}

impl<T: ?Sized> Deferred<Incoming> for CoreVecDecoder<ResourceOwnDecoder<T>> {
    fn take_deferred(&mut self) -> Option<DeferredFn<Incoming>> {
        None
    }
}

impl<T: ?Sized + Send + 'static> Decode for ResourceOwn<T> {
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
pub trait TupleEncode: Encode {}

/// Marker trait for [Decode] tuple types
pub trait TupleDecode: Decode {}

impl Encode for () {
    type Encoder = UnitCodec;
}

impl TupleEncode for () {}

impl Decode for () {
    type Decoder = UnitCodec;
    type ListDecoder = CoreVecDecoder<Self::Decoder>;
}

impl TupleDecode for () {}

macro_rules! impl_tuple_codec {
    ($($vn:ident),+; $($vt:ident),+; $($cn:ident),+; $($ct:ident),+) => {
        impl<$($ct),+> Deferred<Outgoing> for TupleEncoder::<($($ct),+,)>
        where
            $($ct: Deferred<Outgoing> + Default + 'static),+
        {
            fn take_deferred(&mut self) -> Option<DeferredFn<Outgoing>> {
                let Self(($(mut $cn),+,)) = mem::take(self);
                let deferred = [ $($cn.take_deferred()),+ ];
                if deferred.iter().any(Option::is_some) {
                    Some(Box::new(|r, path| Box::pin(handle_deferred_tx(r, deferred, path))))
                } else {
                    None
                }
            }
        }

        impl<E, $($vt),+> Encode for ($($vt),+,)
        where
            E: From<std::io::Error>,
            $(
                $vt: Encode,
                $vt::Encoder: tokio_util::codec::Encoder<$vt, Error = E> + 'static,
            )+
        {
            type Encoder = TupleEncoder::<($($vt::Encoder),+,)>;
        }

        impl<E, $($vt),+> TupleEncode for ($($vt),+,)
        where
            E: From<std::io::Error>,
            $(
                $vt: Encode,
                $vt::Encoder: tokio_util::codec::Encoder<$vt, Error = E> + 'static,
            )+
        {
        }

        impl<'a, E, $($vt),+> Encode for &'a ($($vt),+,)
        where
            E: From<std::io::Error>,
            $(
                $vt: Encode,
                $vt::Encoder: tokio_util::codec::Encoder<&'a $vt, Error = E> + 'static,
            )+
        {
            type Encoder = TupleEncoder::<($($vt::Encoder),+,)>;
        }

        impl<$($vt),+> Deferred<Incoming> for TupleDecoder::<($($vt::Decoder),+,), ($(Option<$vt>),+,)>
        where
            $($vt: Decode),+
        {
            fn take_deferred(&mut self) -> Option<DeferredFn<Incoming>> {
                let ($(mut $cn),+,) = mem::take(self).into_inner();
                let deferred = [ $($cn.take_deferred()),+ ];
                if deferred.iter().any(Option::is_some) {
                    Some(Box::new(|r, path| Box::pin(handle_deferred_rx(r, deferred, path))))
                } else {
                    None
                }
            }
        }

        impl<E, $($vt),+> Decode for ($($vt),+,)
        where
            E: From<std::io::Error>,
            $(
                $vt: Decode + Send + 'static,
                $vt::Decoder: tokio_util::codec::Decoder<Error = E> + Send + 'static,
            )+
        {
            type Decoder = TupleDecoder::<($($vt::Decoder),+,), ($(Option<$vt>),+,)>;
            type ListDecoder = ListDecoder<Self::Decoder>;
        }

        impl<E, $($vt),+> TupleDecode for ($($vt),+,)
        where
            E: From<std::io::Error>,
            $(
                $vt: Decode + Send + 'static,
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
#[derive(Default)]
pub struct FutureEncoder {
    deferred: Option<DeferredFn<Outgoing>>,
}

impl Deferred<Outgoing> for FutureEncoder {
    fn take_deferred(&mut self) -> Option<DeferredFn<Outgoing>> {
        self.deferred.take()
    }
}

impl<T, Fut> tokio_util::codec::Encoder<Fut> for FutureEncoder
where
    T: Encode,
    Fut: Future<Output = T> + Send + 'static,
    std::io::Error: From<<T::Encoder as tokio_util::codec::Encoder<T>>::Error>,
{
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self, item), fields(ty = "future"))]
    fn encode(&mut self, item: Fut, dst: &mut BytesMut) -> std::io::Result<()> {
        // TODO: Check if future is resolved
        dst.reserve(1);
        dst.put_u8(0x00);
        let span = Span::current();
        self.deferred = Some(Box::new(|mut w, path| {
            Box::pin(
                async move {
                    if !path.is_empty() {
                        w = w.index(&path).map_err(std::io::Error::other)?;
                    };
                    let item = item.await;
                    let mut enc = T::Encoder::default();
                    let mut buf = BytesMut::default();
                    enc.encode(item, &mut buf)?;
                    w.write_all(&buf).await?;
                    match enc.take_deferred() {
                        Some(f) => f(w, Vec::default()).await,
                        _ => Ok(()),
                    }
                }
                .instrument(span),
            )
        }));
        Ok(())
    }
}

impl<T> Encode for Pin<Box<dyn Future<Output = T> + Send>>
where
    T: Encode + 'static,
    std::io::Error: From<<T::Encoder as tokio_util::codec::Encoder<T>>::Error>,
{
    type Encoder = FutureEncoder;
}

/// Decoder for `future<T>`
pub struct FutureDecoder<T>
where
    T: Decode,
{
    dec: OptionDecoder<T::Decoder>,
    deferred: Option<DeferredFn<Incoming>>,
    _ty: PhantomData<T>,
}

impl<T> Default for FutureDecoder<T>
where
    T: Decode,
{
    fn default() -> Self {
        Self {
            dec: OptionDecoder::default(),
            deferred: None,
            _ty: PhantomData,
        }
    }
}

impl<T> Deferred<Incoming> for FutureDecoder<T>
where
    T: Decode,
{
    fn take_deferred(&mut self) -> Option<DeferredFn<Incoming>> {
        self.deferred.take()
    }
}

impl<T> tokio_util::codec::Decoder for FutureDecoder<T>
where
    T: Decode + Send + 'static,
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
        self.deferred = Some(Box::new(|mut r, path| {
            Box::pin(
                async move {
                    if !path.is_empty() {
                        r = r.index(&path).map_err(std::io::Error::other)?;
                    };
                    let mut dec = FramedRead::new(r, dec);
                    trace!(?path, "receiving future element");
                    let Some(item) = dec.next().await else {
                        return Err(std::io::ErrorKind::UnexpectedEof.into());
                    };
                    let item = item?;
                    if tx.send(item).is_err() {
                        debug!("future receiver closed, discard data");
                        return Ok(());
                    }
                    if let Some(rx) = dec.decoder_mut().take_deferred() {
                        let buf = mem::take(dec.read_buffer_mut());
                        let mut r = dec.into_inner();
                        if !r.buffer.is_empty() {
                            r.buffer.unsplit(buf);
                        } else {
                            r.buffer = buf;
                        }
                        rx(r, Vec::default()).await?;
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

impl<T> Decode for Pin<Box<dyn Future<Output = T> + Send>>
where
    T: Decode + Send + 'static,
    std::io::Error: From<<T::Decoder as tokio_util::codec::Decoder>::Error>,
{
    type Decoder = FutureDecoder<T>;
    type ListDecoder = ListDecoder<Self::Decoder>;
}

/// Encoder for `stream<T>`
#[derive(Default)]
pub struct StreamEncoder {
    deferred: Option<DeferredFn<Outgoing>>,
}

impl Deferred<Outgoing> for StreamEncoder {
    fn take_deferred(&mut self) -> Option<DeferredFn<Outgoing>> {
        self.deferred.take()
    }
}

impl<T, S> tokio_util::codec::Encoder<S> for StreamEncoder
where
    T: Encode + Send + 'static,
    S: Stream<Item = Vec<T>> + Send + Unpin + 'static,
    std::io::Error: From<<T::Encoder as tokio_util::codec::Encoder<T>>::Error>,
{
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self, items), fields(ty = "stream"))]
    fn encode(&mut self, mut items: S, dst: &mut BytesMut) -> std::io::Result<()> {
        // TODO: Check if stream is resolved
        dst.reserve(1);
        dst.put_u8(0x00);
        let span = Span::current();
        self.deferred = Some(Box::new(|mut w, path| {
            Box::pin(async move {
                if !path.is_empty() {
                    w = w.index(&path).map_err(std::io::Error::other)?;
                };
                let mut enc = T::Encoder::default();
                let mut buf = BytesMut::default();
                let mut tasks = JoinSet::new();
                let mut i = 0_u64;
                loop {
                    select! {
                        chunk = items.next() => {
                            let Some(chunk) = chunk else {
                                trace!("writing stream end");
                                buf.reserve(1);
                                buf.put_u8(0x00);
                                w.write_all(&buf).await?;
                                while let Some(res) = tasks.join_next().await {
                                    trace!(?res, "receiver task finished");
                                    res??;
                                }
                                return Ok(())
                            };
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
                                    let i = i
                                        .try_into()
                                        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
                                    let w = w.index(&[i]).map_err(std::io::Error::other)?;
                                    trace!("spawning transmit task");
                                    tasks.spawn(f(w, Vec::default()));
                                }
                            }
                            i = end;
                        }
                        Some(res) = tasks.join_next() => {
                            trace!(?res, "receiver task finished");
                            res??;
                        }
                        res = w.write(&buf), if !buf.is_empty() => {
                            let n = res?;
                            trace!(?buf, n, "wrote bytes from buffer");
                            buf.advance(n);
                        }
                    }
                }
            }.instrument(span))
        }));
        Ok(())
    }
}

impl<T> Encode for Pin<Box<dyn Stream<Item = Vec<T>> + Send>>
where
    T: Encode + Send + 'static,
    std::io::Error: From<<T::Encoder as tokio_util::codec::Encoder<T>>::Error>,
{
    type Encoder = StreamEncoder;
}

/// Encoder for `stream<list<u8>>`
#[derive(Default)]
pub struct StreamEncoderBytes {
    deferred: Option<DeferredFn<Outgoing>>,
}

impl Deferred<Outgoing> for StreamEncoderBytes {
    fn take_deferred(&mut self) -> Option<DeferredFn<Outgoing>> {
        self.deferred.take()
    }
}

impl<S> tokio_util::codec::Encoder<S> for StreamEncoderBytes
where
    S: Stream<Item = Bytes> + Send + Unpin + 'static,
{
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self, items), fields(ty = "stream<u8>"))]
    fn encode(&mut self, mut items: S, dst: &mut BytesMut) -> std::io::Result<()> {
        // TODO: Check if reader is resolved
        dst.reserve(1);
        dst.put_u8(0x00);
        self.deferred = Some(Box::new(|mut w, path| {
            Box::pin(async move {
                if !path.is_empty() {
                    w = w.index(&path).map_err(std::io::Error::other)?;
                };
                let mut buf = BytesMut::default();
                loop {
                    select! {
                        chunk = items.next() => {
                            let Some(chunk) = chunk else {
                                trace!("writing stream end");
                                buf.reserve(1);
                                buf.put_u8(0x00);
                                return w.write_all(&buf).await
                            };
                            let n = u32::try_from(chunk.len()).map_err(|err| {
                                std::io::Error::new(std::io::ErrorKind::InvalidInput, err)
                            })?;
                            trace!(n, "encoding chunk length");
                            Leb128Encoder.encode(n, &mut buf)?;
                            buf.extend_from_slice(&chunk);
                        }
                        res = w.write(&buf), if !buf.is_empty() => {
                            let n = res?;
                            buf.advance(n);
                        }
                    }
                }
            })
        }));
        Ok(())
    }
}

impl Encode for Pin<Box<dyn Stream<Item = Bytes> + Send>> {
    type Encoder = StreamEncoderBytes;
}

/// Encoder for `stream<list<u8>>` with [`AsyncRead`] support
#[derive(Default)]
pub struct StreamEncoderRead {
    deferred: Option<DeferredFn<Outgoing>>,
}

impl Deferred<Outgoing> for StreamEncoderRead {
    fn take_deferred(&mut self) -> Option<DeferredFn<Outgoing>> {
        self.deferred.take()
    }
}

impl<S> tokio_util::codec::Encoder<S> for StreamEncoderRead
where
    S: AsyncRead + Send + Unpin + 'static,
{
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self, items), fields(ty = "stream<u8>"))]
    fn encode(&mut self, mut items: S, dst: &mut BytesMut) -> std::io::Result<()> {
        // TODO: Check if reader is resolved
        dst.reserve(1);
        dst.put_u8(0x00);
        self.deferred = Some(Box::new(|mut w, path| {
            Box::pin(async move {
                if !path.is_empty() {
                    w = w.index(&path).map_err(std::io::Error::other)?;
                };
                let mut buf = BytesMut::default();
                let mut chunk = BytesMut::default();
                loop {
                    select! {
                        res = items.read_buf(&mut chunk) => {
                            let n = res?;
                            if n == 0 {
                                trace!("writing stream end");
                                buf.reserve(1);
                                buf.put_u8(0x00);
                                return w.write_all(&buf).await
                            }
                            let n = u32::try_from(n).map_err(|err| {
                                std::io::Error::new(std::io::ErrorKind::InvalidInput, err)
                            })?;
                            trace!(n, "encoding chunk length");
                            Leb128Encoder.encode(n, &mut buf)?;
                            buf.extend_from_slice(&chunk);
                            chunk.clear();
                        }
                        res = w.write(&buf), if !buf.is_empty() => {
                            let n = res?;
                            buf.advance(n);
                        }
                    }
                }
            })
        }));
        Ok(())
    }
}

impl Encode for Pin<Box<dyn AsyncRead + Send>> {
    type Encoder = StreamEncoderRead;
}

impl<T> Encode for std::io::Cursor<T>
where
    T: AsRef<[u8]> + Send + Unpin + 'static,
{
    type Encoder = StreamEncoderRead;
}

impl Encode for tokio::io::Empty {
    type Encoder = StreamEncoderRead;
}

#[cfg(feature = "io-std")]
impl Encode for tokio::io::Stdin {
    type Encoder = StreamEncoderRead;
}

#[cfg(feature = "fs")]
impl Encode for tokio::fs::File {
    type Encoder = StreamEncoderRead;
}

#[cfg(feature = "net")]
impl Encode for tokio::net::TcpStream {
    type Encoder = StreamEncoderRead;
}

#[cfg(all(unix, feature = "net"))]
impl Encode for tokio::net::UnixStream {
    type Encoder = StreamEncoderRead;
}

#[cfg(all(unix, feature = "net"))]
impl Encode for tokio::net::unix::pipe::Receiver {
    type Encoder = StreamEncoderRead;
}

/// Decoder for `stream<T>`
pub struct StreamDecoder<T>
where
    T: Decode,
{
    dec: T::ListDecoder,
    deferred: Option<DeferredFn<Incoming>>,
    _ty: PhantomData<T>,
}

impl<T> Default for StreamDecoder<T>
where
    T: Decode,
{
    fn default() -> Self {
        Self {
            dec: T::ListDecoder::default(),
            deferred: None,
            _ty: PhantomData,
        }
    }
}

impl<T> Deferred<Incoming> for StreamDecoder<T>
where
    T: Decode,
{
    fn take_deferred(&mut self) -> Option<DeferredFn<Incoming>> {
        self.deferred.take()
    }
}

#[instrument(level = "trace", skip(dec, r, tx), ret)]
async fn handle_deferred_stream<C, T>(
    dec: C,
    mut r: Incoming,
    path: Vec<usize>,
    tx: mpsc::Sender<Vec<T>>,
) -> std::io::Result<()>
where
    C: tokio_util::codec::Decoder<Item = T> + Deferred<Incoming>,
    std::io::Error: From<C::Error>,
{
    let dec = ListDecoder::new(dec);
    if !path.is_empty() {
        r = r.index(&path).map_err(std::io::Error::other)?;
    };
    let mut framed = FramedRead::new(r, dec);
    let mut tasks = JoinSet::new();
    let mut i = 0_usize;
    loop {
        trace!("receiving pending stream chunk");
        select! {
            Some(chunk) = framed.next() => {
                let chunk = chunk?;
                if chunk.is_empty() {
                    trace!("received stream end");
                    while let Some(res) = tasks.join_next().await {
                        res??;
                    }
                    return Ok(())
                }
                let end = i.checked_add(chunk.len()).ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "stream element index would overflow usize",
                    )
                })?;
                trace!(i, end, "received stream chunk");
                if tx.send(chunk).await.is_err() {
                    debug!("stream receiver closed, discard data");
                    return Ok(())
                }
                for (i, deferred) in zip(i.., mem::take(&mut framed.decoder_mut().deferred)) {
                    if let Some(deferred) = deferred {
                        let r = framed.get_ref().index(&[i]).map_err(std::io::Error::other)?;
                        trace!("spawning receive task");
                        tasks.spawn(deferred(r, Vec::default()));
                    }
                }
                i = end;
            },
            Some(res) = tasks.join_next() => {
                trace!(?res, "receiver task finished");
                res??;
            }
            else => {
                return Ok(());
            }
        }
    }
}

impl<T> tokio_util::codec::Decoder for StreamDecoder<T>
where
    T: Decode + Send + 'static,
    T::ListDecoder: Deferred<Incoming>,
    <T::Decoder as tokio_util::codec::Decoder>::Error: Send,
    std::io::Error: From<<T::Decoder as tokio_util::codec::Decoder>::Error>,
{
    type Item = Pin<Box<dyn Stream<Item = Vec<T>> + Send>>;
    type Error = <<T as Decode>::ListDecoder as tokio_util::codec::Decoder>::Error;

    #[instrument(level = "trace", skip(self), fields(ty = "stream"))]
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let Some(chunk) = self.dec.decode(src)? else {
            return Ok(None);
        };
        if !chunk.is_empty() {
            self.deferred = self.dec.take_deferred();
            return Ok(Some(Box::pin(stream::iter([chunk]))));
        }

        // stream is pending
        let (tx, rx) = mpsc::channel(128);
        self.deferred = Some(Box::new(|r, path| {
            Box::pin(
                async move { handle_deferred_stream(T::Decoder::default(), r, path, tx).await },
            )
        }));
        Ok(Some(Box::pin(ReceiverStream::new(rx))))
    }
}

impl<T> Decode for Pin<Box<dyn Stream<Item = Vec<T>> + Send>>
where
    T: Decode + Send + 'static,
    T::ListDecoder: Deferred<Incoming> + Send,
    <T::Decoder as tokio_util::codec::Decoder>::Error: Send,
    std::io::Error: From<<T::Decoder as tokio_util::codec::Decoder>::Error>,
{
    type Decoder = StreamDecoder<T>;
    type ListDecoder = ListDecoder<Self::Decoder>;
}

/// Decoder for `stream<list<u8>>`
#[derive(Default)]
pub struct StreamDecoderBytes {
    dec: CoreVecDecoderBytes,
    deferred: Option<DeferredFn<Incoming>>,
}

impl Deferred<Incoming> for StreamDecoderBytes {
    fn take_deferred(&mut self) -> Option<DeferredFn<Incoming>> {
        self.deferred.take()
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
        if !chunk.is_empty() {
            return Ok(Some(Box::pin(stream::iter([chunk]))));
        }

        // stream is pending
        let (tx, rx) = mpsc::channel(128);
        let dec = mem::take(&mut self.dec);
        let span = Span::current();
        self.deferred = Some(Box::new(|mut r, path| {
            Box::pin(
                async move {
                    if !path.is_empty() {
                        r = r.index(&path).map_err(std::io::Error::other)?;
                    };
                    let mut framed = FramedRead::new(r, dec);
                    trace!(?path, "receiving pending byte stream chunk");
                    while let Some(chunk) = framed.next().await {
                        let chunk = chunk?;
                        if chunk.is_empty() {
                            trace!("received stream end");
                            return Ok(());
                        }
                        trace!(?chunk, "received pending byte stream chunk");
                        if tx.send(chunk).await.is_err() {
                            debug!("stream receiver closed, discard data");
                            return Ok(());
                        }
                    }
                    Ok(())
                }
                .instrument(span),
            )
        }));
        Ok(Some(Box::pin(ReceiverStream::new(rx))))
    }
}

impl Decode for Pin<Box<dyn Stream<Item = Bytes> + Send>> {
    type Decoder = StreamDecoderBytes;
    type ListDecoder = ListDecoder<Self::Decoder>;
}

/// Decoder for `stream<list<u8>>` with [`AsyncRead`] support
#[derive(Default)]
pub struct StreamDecoderRead {
    dec: CoreVecDecoderBytes,
    deferred: Option<DeferredFn<Incoming>>,
}

impl Deferred<Incoming> for StreamDecoderRead {
    fn take_deferred(&mut self) -> Option<DeferredFn<Incoming>> {
        self.deferred.take()
    }
}

impl tokio_util::codec::Decoder for StreamDecoderRead {
    type Item = Pin<Box<dyn AsyncRead + Send>>;
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self), fields(ty = "stream<u8>"))]
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let Some(chunk) = self.dec.decode(src)? else {
            return Ok(None);
        };
        if !chunk.is_empty() {
            return Ok(Some(Box::pin(std::io::Cursor::new(chunk))));
        }

        // stream is pending
        let (tx, rx) = mpsc::channel(128);
        let dec = mem::take(&mut self.dec);
        self.deferred = Some(Box::new(|mut r, path| {
            Box::pin(async move {
                if !path.is_empty() {
                    r = r.index(&path).map_err(std::io::Error::other)?;
                };
                let mut framed = FramedRead::new(r, dec);
                trace!("receiving pending byte stream chunk");
                while let Some(chunk) = framed.next().await {
                    let chunk = chunk?;
                    if chunk.is_empty() {
                        trace!("received stream end");
                        return Ok(());
                    }
                    trace!(?chunk, "received byte stream chunk");
                    if tx.send(std::io::Result::Ok(chunk)).await.is_err() {
                        debug!("stream receiver closed, discard data");
                        return Ok(());
                    }
                }
                Ok(())
            })
        }));
        Ok(Some(Box::pin(StreamReader::new(ReceiverStream::new(rx)))))
    }
}

impl Decode for Pin<Box<dyn AsyncRead + Send>> {
    type Decoder = StreamDecoderRead;
    type ListDecoder = ListDecoder<Self::Decoder>;
}

#[cfg(test)]
mod tests {
    use anyhow::bail;

    use super::*;

    #[test_log::test(tokio::test)]
    async fn codec() -> anyhow::Result<()> {
        let mut buf = BytesMut::new();
        let mut enc = <(u8, u32) as Encode>::Encoder::default();
        enc.encode((0x42, 0x42), &mut buf)?;
        if let Some(_f) = Deferred::<Outgoing>::take_deferred(&mut enc) {
            bail!("no deferred write should have been returned");
        }
        assert_eq!(buf.as_ref(), b"\x42\x42");
        Ok(())
    }

    #[test]
    fn canonical_nan_f32() {
        let mut enc = <f32 as Encode>::Encoder::default();

        // A non-canonical (e.g. signalling) `NaN` is canonicalized on encode.
        let mut buf = BytesMut::new();
        enc.encode(f32::from_bits(0x7f80_0001), &mut buf).unwrap();
        assert_eq!(buf.as_ref(), CANONICAL_NAN_F32.to_le_bytes());

        // A negative `NaN` is canonicalized to the (positive) canonical `NaN`.
        let mut buf = BytesMut::new();
        enc.encode(f32::from_bits(0xffc0_0000), &mut buf).unwrap();
        assert_eq!(buf.as_ref(), CANONICAL_NAN_F32.to_le_bytes());

        // Non-`NaN` values are encoded unchanged.
        let mut buf = BytesMut::new();
        enc.encode(1.5_f32, &mut buf).unwrap();
        assert_eq!(buf.as_ref(), 1.5_f32.to_bits().to_le_bytes());
    }

    #[test]
    fn canonical_nan_f64() {
        let mut enc = <f64 as Encode>::Encoder::default();

        let mut buf = BytesMut::new();
        enc.encode(f64::from_bits(0x7ff0_0000_0000_0001), &mut buf)
            .unwrap();
        assert_eq!(buf.as_ref(), CANONICAL_NAN_F64.to_le_bytes());

        let mut buf = BytesMut::new();
        enc.encode(f64::from_bits(0xfff8_0000_0000_0000), &mut buf)
            .unwrap();
        assert_eq!(buf.as_ref(), CANONICAL_NAN_F64.to_le_bytes());

        let mut buf = BytesMut::new();
        enc.encode(1.5_f64, &mut buf).unwrap();
        assert_eq!(buf.as_ref(), 1.5_f64.to_bits().to_le_bytes());
    }
}
