use core::any::TypeId;
use core::fmt::{self, Debug};
use core::future::pending;
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;
use core::mem;
use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use core::str;
use core::task::{Context, Poll, ready};

use std::collections::BTreeMap;

use bytes::{Buf, BufMut as _, Bytes, BytesMut};
use futures::stream;
use futures::{FutureExt as _, Stream, StreamExt as _};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite};
use tokio::sync::{mpsc, oneshot};
use tokio_util::codec::{Decoder as _, Encoder as _};
use tokio_util::io::{StreamReader, poll_read_buf};
use tokio_util::sync::PollSender;
use tracing::{error, instrument};
use wasm_tokio::cm::{
    BoolCodec, F32Codec, F64Codec, OptionDecoder, OptionEncoder, PrimValEncoder, ResultDecoder,
    ResultEncoder, S8Codec, S16Codec, S32Codec, S64Codec, TupleDecoder, TupleEncoder, U8Codec,
    U16Codec, U32Codec, U64Codec,
};
use wasm_tokio::{
    CoreNameDecoder, CoreNameEncoder, CoreVecDecoder, CoreVecDecoderBytes, CoreVecEncoderBytes,
    DEFAULT_MAX_INITIAL_CAPACITY, Leb128DecoderI8, Leb128DecoderI16, Leb128DecoderI32,
    Leb128DecoderI64, Leb128DecoderI128, Leb128DecoderU8, Leb128DecoderU16, Leb128DecoderU32,
    Leb128DecoderU64, Leb128DecoderU128, Leb128Encoder, Utf8Codec, put_u32_leb128,
};

use crate::{
    encode_close, encode_frames, error_data_after_end, error_receiver_closed, error_unknown_path,
};

/// Size of chunks read from byte streams
pub const READ_CHUNK_SIZE: usize = 8192;

fn encode_ready_bytes(chunks: &[impl AsRef<[u8]>], dst: &mut BytesMut) -> std::io::Result<()> {
    let n = chunks.iter().map(|chunk| chunk.as_ref().len()).sum();
    let len = u32::try_from(n)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
    dst.reserve(1);
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

/// Encoder of values, which transmits the asynchronous values nested in the last encoded value
pub trait Transmit {
    /// Encodes the frames, which are ready, of the asynchronous values nested in the last encoded
    /// value, whose paths are nested in `path`, into `dst`, returning [`Poll::Ready`] once all of
    /// them are closed.
    ///
    /// Returns [`Poll::Pending`] only after encoding a frame or registering `cx` for wake-up.
    fn poll_transmit(
        &mut self,
        cx: &mut Context<'_>,
        path: &mut Vec<usize>,
        dst: &mut BytesMut,
    ) -> Poll<std::io::Result<()>>;

    /// Returns whether the paths of all asynchronous values nested in the last encoded value are
    /// closed
    fn is_closed(&self) -> bool;
}

impl<T: Transmit + ?Sized> Transmit for Box<T> {
    fn poll_transmit(
        &mut self,
        cx: &mut Context<'_>,
        path: &mut Vec<usize>,
        dst: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        (**self).poll_transmit(cx, path, dst)
    }

    fn is_closed(&self) -> bool {
        (**self).is_closed()
    }
}

impl<T: Transmit> Transmit for BTreeMap<usize, T> {
    fn poll_transmit(
        &mut self,
        cx: &mut Context<'_>,
        path: &mut Vec<usize>,
        dst: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        let mut err = None;
        self.retain(|i, t| {
            if err.is_some() {
                return true;
            }
            path.push(*i);
            let res = t.poll_transmit(cx, path, dst);
            path.pop();
            match res {
                Poll::Ready(Ok(())) => false,
                Poll::Ready(Err(e)) => {
                    err = Some(e);
                    true
                }
                Poll::Pending => true,
            }
        });
        if let Some(err) = err {
            return Poll::Ready(Err(err));
        }
        if self.is_empty() {
            Poll::Ready(Ok(()))
        } else {
            Poll::Pending
        }
    }

    fn is_closed(&self) -> bool {
        self.is_empty()
    }
}

/// Decoder of values, which receives the asynchronous values nested in the last decoded value
pub trait Receive {
    /// Receives `src`, the data of a frame on `path` relative to the last decoded value, which
    /// closes `path` if empty.
    ///
    /// Takes all of `src` once ready, returns [`Poll::Pending`] without taking any of it otherwise.
    fn poll_receive(
        &mut self,
        cx: &mut Context<'_>,
        path: &[usize],
        src: &mut BytesMut,
    ) -> Poll<std::io::Result<()>>;

    /// Returns whether the paths of all asynchronous values nested in the last decoded value are
    /// closed
    fn is_closed(&self) -> bool;
}

impl<T: Receive + ?Sized> Receive for Box<T> {
    fn poll_receive(
        &mut self,
        cx: &mut Context<'_>,
        path: &[usize],
        src: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        (**self).poll_receive(cx, path, src)
    }

    fn is_closed(&self) -> bool {
        (**self).is_closed()
    }
}

impl<T: Receive> Receive for BTreeMap<usize, T> {
    fn poll_receive(
        &mut self,
        cx: &mut Context<'_>,
        path: &[usize],
        src: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        let Some((i, rest)) = path.split_first() else {
            return Poll::Ready(Err(error_unknown_path(path)));
        };
        let Some(r) = self.get_mut(i) else {
            return Poll::Ready(Err(error_unknown_path(path)));
        };
        ready!(r.poll_receive(cx, rest, src))?;
        if r.is_closed() {
            self.remove(i);
        }
        Poll::Ready(Ok(()))
    }

    fn is_closed(&self) -> bool {
        self.is_empty()
    }
}

macro_rules! sync_transmit {
    () => {
        fn poll_transmit(
            &mut self,
            _cx: &mut Context<'_>,
            _path: &mut Vec<usize>,
            _dst: &mut BytesMut,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn is_closed(&self) -> bool {
            true
        }
    };
}

macro_rules! sync_receive {
    () => {
        fn poll_receive(
            &mut self,
            _cx: &mut Context<'_>,
            path: &[usize],
            _src: &mut BytesMut,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Err(error_unknown_path(path)))
        }

        fn is_closed(&self) -> bool {
            true
        }
    };
}

macro_rules! impl_sync_decoder {
    ($($t:ty),+ $(,)?) => {
        $(
            impl Transmit for $t {
                sync_transmit!();
            }

            impl Receive for $t {
                sync_receive!();
            }
        )+
    };
}

impl_sync_decoder!(
    BoolCodec,
    S8Codec,
    U8Codec,
    S16Codec,
    U16Codec,
    S32Codec,
    U32Codec,
    S64Codec,
    U64Codec,
    F32Codec,
    F64Codec,
    CoreNameDecoder,
    CoreVecDecoderBytes,
    Utf8Codec,
    Leb128DecoderI8,
    Leb128DecoderU8,
    Leb128DecoderI16,
    Leb128DecoderU16,
    Leb128DecoderI32,
    Leb128DecoderU32,
    Leb128DecoderI64,
    Leb128DecoderU64,
    Leb128DecoderI128,
    Leb128DecoderU128,
    UnitCodec,
    ListDecoderU8,
    CoreVecDecoder<BoolCodec>,
    CoreVecDecoder<S8Codec>,
    CoreVecDecoder<U8Codec>,
    CoreVecDecoder<S16Codec>,
    CoreVecDecoder<U16Codec>,
    CoreVecDecoder<S32Codec>,
    CoreVecDecoder<U32Codec>,
    CoreVecDecoder<S64Codec>,
    CoreVecDecoder<U64Codec>,
    CoreVecDecoder<F32Codec>,
    CoreVecDecoder<F64Codec>,
    CoreVecDecoder<CoreNameDecoder>,
    CoreVecDecoder<CoreVecDecoderBytes>,
    CoreVecDecoder<Utf8Codec>,
    CoreVecDecoder<Leb128DecoderI8>,
    CoreVecDecoder<Leb128DecoderU8>,
    CoreVecDecoder<Leb128DecoderI16>,
    CoreVecDecoder<Leb128DecoderU16>,
    CoreVecDecoder<Leb128DecoderI32>,
    CoreVecDecoder<Leb128DecoderU32>,
    CoreVecDecoder<Leb128DecoderI64>,
    CoreVecDecoder<Leb128DecoderU64>,
    CoreVecDecoder<Leb128DecoderI128>,
    CoreVecDecoder<Leb128DecoderU128>,
    CoreVecDecoder<UnitCodec>,
);

macro_rules! impl_sync_encoder {
    ($($t:ty),+ $(,)?) => {
        $(
            impl Transmit for $t {
                sync_transmit!();
            }
        )+
    };
}

impl_sync_encoder!(
    CoreNameEncoder,
    CoreVecEncoderBytes,
    PrimValEncoder,
    Utf8BytesEncoder,
    Leb128Encoder,
    ResourceEncoder,
);

/// Codec for synchronous values
///
/// This is a wrapper struct, which provides no-op [Transmit] and [Receive] implementations
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

impl<T> Transmit for SyncCodec<T> {
    sync_transmit!();
}

impl<T> Receive for SyncCodec<T> {
    sync_receive!();
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

/// Codec of encoded values without asynchronous values nested in them
///
/// Decodes all data of a path once it is closed.
#[derive(Clone, Copy, Debug, Default)]
pub struct RawCodec;

impl Transmit for RawCodec {
    sync_transmit!();
}

impl Receive for RawCodec {
    sync_receive!();
}

impl<T: AsRef<[u8]>> tokio_util::codec::Encoder<T> for RawCodec {
    type Error = std::io::Error;

    fn encode(&mut self, item: T, dst: &mut BytesMut) -> Result<(), Self::Error> {
        dst.extend_from_slice(item.as_ref());
        Ok(())
    }
}

impl tokio_util::codec::Decoder for RawCodec {
    type Item = Bytes;
    type Error = std::io::Error;

    fn decode(&mut self, _src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        Ok(None)
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        Ok(Some(src.split().freeze()))
    }
}

/// Defines value encoding
pub trait Encode: Sized {
    /// Encoder used to encode the value
    type Encoder: tokio_util::codec::Encoder<Self, Error = std::io::Error>
        + Transmit
        + Default
        + Send;

    /// Encodes `items`, returning the encoders of the pending ones keyed by their indices
    #[instrument(level = "trace", skip(items))]
    fn encode_iter<I>(
        items: I,
        dst: &mut BytesMut,
    ) -> std::io::Result<BTreeMap<usize, Self::Encoder>>
    where
        I: IntoIterator + AsRef<[Self]>,
        I::IntoIter: ExactSizeIterator,
        Self::Encoder: tokio_util::codec::Encoder<I::Item, Error = std::io::Error>,
    {
        let items = items.into_iter();
        dst.reserve(items.len());
        let mut enc = Self::Encoder::default();
        let mut nested = BTreeMap::default();
        for (i, item) in items.enumerate() {
            tokio_util::codec::Encoder::<I::Item>::encode(&mut enc, item, dst)?;
            if !enc.is_closed() {
                nested.insert(i, mem::take(&mut enc));
            }
        }
        Ok(nested)
    }
}

#[instrument(level = "trace", skip(items), fields(ty = "list"))]
fn encode_list<T, I>(items: I, dst: &mut BytesMut) -> std::io::Result<BTreeMap<usize, T::Encoder>>
where
    T: Encode,
    I: IntoIterator + AsRef<[T]>,
    I::IntoIter: ExactSizeIterator,
    T::Encoder: tokio_util::codec::Encoder<I::Item, Error = std::io::Error>,
{
    let n = u32::try_from(items.as_ref().len())
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
    dst.reserve(5);
    Leb128Encoder.encode(n, dst)?;
    T::encode_iter(items, dst)
}

/// Defines value decoding
pub trait Decode: Sized {
    /// Decoder used to decode value
    type Decoder: tokio_util::codec::Decoder<Item = Self, Error = std::io::Error>
        + Receive
        + Default
        + Send
        + 'static;
    /// Decoder used to decode lists of value
    type ListDecoder: tokio_util::codec::Decoder<Item = Vec<Self>, Error = std::io::Error>
        + Default
        + 'static;
}

impl<T: Transmit> Transmit for OptionEncoder<T> {
    fn poll_transmit(
        &mut self,
        cx: &mut Context<'_>,
        path: &mut Vec<usize>,
        dst: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        self.0.poll_transmit(cx, path, dst)
    }

    fn is_closed(&self) -> bool {
        self.0.is_closed()
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
    T::Encoder: tokio_util::codec::Encoder<&'a T, Error = std::io::Error>,
{
    type Encoder = OptionEncoder<T::Encoder>;
}

impl<T: Receive> Receive for OptionDecoder<T> {
    fn poll_receive(
        &mut self,
        cx: &mut Context<'_>,
        path: &[usize],
        src: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        self.get_mut().poll_receive(cx, path, src)
    }

    fn is_closed(&self) -> bool {
        self.get_ref().is_closed()
    }
}

impl<T> Decode for Option<T>
where
    T: Decode,
{
    type Decoder = OptionDecoder<T::Decoder>;
    type ListDecoder = ListDecoder<Self::Decoder>;
}

impl<O: Transmit, E: Transmit> Transmit for ResultEncoder<O, E> {
    fn poll_transmit(
        &mut self,
        cx: &mut Context<'_>,
        path: &mut Vec<usize>,
        dst: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        ready!(self.ok.poll_transmit(cx, path, dst))?;
        self.err.poll_transmit(cx, path, dst)
    }

    fn is_closed(&self) -> bool {
        self.ok.is_closed() && self.err.is_closed()
    }
}

impl<O, E> Encode for Result<O, E>
where
    O: Encode,
    E: Encode,
{
    type Encoder = ResultEncoder<O::Encoder, E::Encoder>;
}

impl<'a, O, E> Encode for &'a Result<O, E>
where
    O: Encode,
    O::Encoder: tokio_util::codec::Encoder<&'a O, Error = std::io::Error>,
    E: Encode,
    E::Encoder: tokio_util::codec::Encoder<&'a E, Error = std::io::Error>,
{
    type Encoder = ResultEncoder<O::Encoder, E::Encoder>;
}

impl<O: Receive, E: Receive> Receive for ResultDecoder<O, E> {
    fn poll_receive(
        &mut self,
        cx: &mut Context<'_>,
        path: &[usize],
        src: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        let (ok, err) = self.get_mut();
        if ok.is_closed() {
            err.poll_receive(cx, path, src)
        } else {
            ok.poll_receive(cx, path, src)
        }
    }

    fn is_closed(&self) -> bool {
        let (ok, err) = self.get_ref();
        ok.is_closed() && err.is_closed()
    }
}

impl<O, E> Decode for Result<O, E>
where
    O: Decode,
    E: Decode,
{
    type Decoder = ResultDecoder<O::Decoder, E::Decoder>;
    type ListDecoder = ListDecoder<Self::Decoder>;
}

/// Encoder for `list<T>` and `list<T, N>`
pub struct ListEncoder<E> {
    nested: BTreeMap<usize, E>,
}

impl<E> Default for ListEncoder<E> {
    fn default() -> Self {
        Self {
            nested: BTreeMap::default(),
        }
    }
}

impl<E: Transmit> Transmit for ListEncoder<E> {
    fn poll_transmit(
        &mut self,
        cx: &mut Context<'_>,
        path: &mut Vec<usize>,
        dst: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        self.nested.poll_transmit(cx, path, dst)
    }

    fn is_closed(&self) -> bool {
        self.nested.is_closed()
    }
}

impl<T> tokio_util::codec::Encoder<Vec<T>> for ListEncoder<T::Encoder>
where
    T: Encode,
{
    type Error = std::io::Error;

    fn encode(&mut self, items: Vec<T>, dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.nested = encode_list(items, dst)?;
        Ok(())
    }
}

impl<'a, T> tokio_util::codec::Encoder<&'a Vec<T>> for ListEncoder<T::Encoder>
where
    T: Encode,
    T::Encoder: tokio_util::codec::Encoder<&'a T, Error = std::io::Error>,
{
    type Error = std::io::Error;

    fn encode(&mut self, items: &'a Vec<T>, dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.nested = encode_list(items, dst)?;
        Ok(())
    }
}

impl<'a, 'b, T> tokio_util::codec::Encoder<&'a &'b Vec<T>> for ListEncoder<T::Encoder>
where
    T: Encode,
    T::Encoder: tokio_util::codec::Encoder<&'b T, Error = std::io::Error>,
{
    type Error = std::io::Error;

    fn encode(&mut self, items: &'a &'b Vec<T>, dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.nested = encode_list(*items, dst)?;
        Ok(())
    }
}

impl<'a, T> tokio_util::codec::Encoder<&'a [T]> for ListEncoder<T::Encoder>
where
    T: Encode,
    T::Encoder: tokio_util::codec::Encoder<&'a T, Error = std::io::Error>,
{
    type Error = std::io::Error;

    fn encode(&mut self, items: &'a [T], dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.nested = encode_list(items, dst)?;
        Ok(())
    }
}

impl<'a, 'b, T> tokio_util::codec::Encoder<&'a &'b [T]> for ListEncoder<T::Encoder>
where
    T: Encode,
    T::Encoder: tokio_util::codec::Encoder<&'b T, Error = std::io::Error>,
{
    type Error = std::io::Error;

    fn encode(&mut self, items: &'a &'b [T], dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.nested = encode_list(*items, dst)?;
        Ok(())
    }
}

impl<T> Encode for Vec<T>
where
    T: Encode,
{
    type Encoder = ListEncoder<T::Encoder>;
}

impl<'a, T> Encode for &'a Vec<T>
where
    T: Encode,
    T::Encoder: tokio_util::codec::Encoder<&'a T, Error = std::io::Error>,
{
    type Encoder = ListEncoder<T::Encoder>;
}

impl<'a, T> Encode for &'a [T]
where
    T: Encode,
    T::Encoder: tokio_util::codec::Encoder<&'a T, Error = std::io::Error>,
{
    type Encoder = ListEncoder<T::Encoder>;
}

/// Decoder for `list<T>`
pub struct ListDecoder<T>
where
    T: tokio_util::codec::Decoder,
{
    dec: T,
    ret: Vec<T::Item>,
    cap: usize,
    nested: BTreeMap<usize, T>,
}

impl<T> Default for ListDecoder<T>
where
    T: tokio_util::codec::Decoder + Default,
{
    fn default() -> Self {
        Self {
            dec: T::default(),
            ret: Vec::default(),
            cap: 0,
            nested: BTreeMap::default(),
        }
    }
}

impl<T> Receive for ListDecoder<T>
where
    T: tokio_util::codec::Decoder + Receive,
{
    fn poll_receive(
        &mut self,
        cx: &mut Context<'_>,
        path: &[usize],
        src: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        self.nested.poll_receive(cx, path, src)
    }

    fn is_closed(&self) -> bool {
        self.nested.is_closed()
    }
}

impl<T> tokio_util::codec::Decoder for ListDecoder<T>
where
    T: tokio_util::codec::Decoder + Receive + Default,
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
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
            self.ret = Vec::with_capacity(
                len.min(DEFAULT_MAX_INITIAL_CAPACITY / mem::size_of::<T::Item>().max(1)),
            );
            self.cap = len;
        }
        while self.cap > 0 {
            let Some(v) = self.dec.decode(src)? else {
                return Ok(None);
            };
            if !self.dec.is_closed() {
                self.nested.insert(self.ret.len(), mem::take(&mut self.dec));
            }
            self.ret.push(v);
            self.cap -= 1;
        }
        Ok(Some(mem::take(&mut self.ret)))
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match self.decode(src)? {
            Some(v) => Ok(Some(v)),
            None if self.cap == 0 && src.is_empty() => Ok(None),
            None => Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into()),
        }
    }
}

impl<T> Decode for Vec<T>
where
    T: Decode + Send,
    T::ListDecoder: Receive + Send,
{
    type Decoder = T::ListDecoder;
    type ListDecoder = ListDecoder<Self::Decoder>;
}

impl<T, const N: usize> tokio_util::codec::Encoder<[T; N]> for ListEncoder<T::Encoder>
where
    T: Encode,
{
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self, items), fields(ty = "array"))]
    fn encode(&mut self, items: [T; N], dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.nested = T::encode_iter(items, dst)?;
        Ok(())
    }
}

impl<'a, T, const N: usize> tokio_util::codec::Encoder<&'a [T; N]> for ListEncoder<T::Encoder>
where
    T: Encode,
    T::Encoder: tokio_util::codec::Encoder<&'a T, Error = std::io::Error>,
{
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self, items), fields(ty = "array"))]
    fn encode(&mut self, items: &'a [T; N], dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.nested = T::encode_iter(items, dst)?;
        Ok(())
    }
}

impl<'a, 'b, T, const N: usize> tokio_util::codec::Encoder<&'a &'b [T; N]>
    for ListEncoder<T::Encoder>
where
    T: Encode,
    T::Encoder: tokio_util::codec::Encoder<&'b T, Error = std::io::Error>,
{
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self, items), fields(ty = "array"))]
    fn encode(&mut self, items: &'a &'b [T; N], dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.nested = T::encode_iter(*items, dst)?;
        Ok(())
    }
}

impl<T, const N: usize> Encode for [T; N]
where
    T: Encode,
{
    type Encoder = ListEncoder<T::Encoder>;
}

impl<'a, T, const N: usize> Encode for &'a [T; N]
where
    T: Encode,
    T::Encoder: tokio_util::codec::Encoder<&'a T, Error = std::io::Error>,
{
    type Encoder = ListEncoder<T::Encoder>;
}

/// Decoder for `list<T, N>`
pub struct ArrayDecoder<T, const N: usize>
where
    T: tokio_util::codec::Decoder,
{
    dec: T,
    ret: Vec<T::Item>,
    nested: BTreeMap<usize, T>,
}

impl<T, const N: usize> Default for ArrayDecoder<T, N>
where
    T: tokio_util::codec::Decoder + Default,
{
    fn default() -> Self {
        Self {
            dec: T::default(),
            ret: Vec::default(),
            nested: BTreeMap::default(),
        }
    }
}

impl<T, const N: usize> Receive for ArrayDecoder<T, N>
where
    T: tokio_util::codec::Decoder + Receive,
{
    fn poll_receive(
        &mut self,
        cx: &mut Context<'_>,
        path: &[usize],
        src: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        self.nested.poll_receive(cx, path, src)
    }

    fn is_closed(&self) -> bool {
        self.nested.is_closed()
    }
}

impl<T, const N: usize> tokio_util::codec::Decoder for ArrayDecoder<T, N>
where
    T: tokio_util::codec::Decoder + Receive + Default,
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
            let Some(v) = self.dec.decode(src)? else {
                return Ok(None);
            };
            if !self.dec.is_closed() {
                self.nested.insert(self.ret.len(), mem::take(&mut self.dec));
            }
            self.ret.push(v);
        }
        let Ok(ret) = mem::take(&mut self.ret).try_into() else {
            unreachable!("array decoder decoded exactly `N` elements")
        };
        Ok(Some(ret))
    }
}

impl<T, const N: usize> Decode for [T; N]
where
    T: Decode + Send,
{
    type Decoder = ArrayDecoder<T::Decoder, N>;
    type ListDecoder = ListDecoder<Self::Decoder>;
}

macro_rules! impl_copy_codec {
    ($t:ty, $c:tt) => {
        impl Encode for $t {
            type Encoder = $c;

            #[instrument(level = "trace", skip(items))]
            fn encode_iter<I>(
                items: I,
                dst: &mut BytesMut,
            ) -> Result<
                BTreeMap<usize, Self::Encoder>,
                <Self::Encoder as tokio_util::codec::Encoder<I::Item>>::Error,
            >
            where
                I: IntoIterator + AsRef<[Self]>,
                I::IntoIter: ExactSizeIterator,
                Self::Encoder: tokio_util::codec::Encoder<I::Item>,
            {
                let items = items.as_ref();
                dst.reserve(items.len());
                for item in items {
                    tokio_util::codec::Encoder::<$t>::encode(&mut $c, *item, dst)?;
                }
                Ok(BTreeMap::default())
            }
        }

        impl Encode for &$t {
            type Encoder = $c;
        }

        impl Decode for $t {
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

impl Encode for u8 {
    type Encoder = U8Codec;

    #[instrument(level = "trace", skip(items))]
    fn encode_iter<I>(
        items: I,
        dst: &mut BytesMut,
    ) -> std::io::Result<BTreeMap<usize, Self::Encoder>>
    where
        I: IntoIterator + AsRef<[Self]>,
        I::IntoIter: ExactSizeIterator,
        Self::Encoder: tokio_util::codec::Encoder<I::Item, Error = std::io::Error>,
    {
        dst.extend_from_slice(items.as_ref());
        Ok(BTreeMap::default())
    }
}

impl Encode for &u8 {
    type Encoder = U8Codec;
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

    fn decode_eof(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let Some(buf) = self.0.decode_eof(src)? else {
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

impl<T> Encode for Utf8Bytes<T>
where
    T: AsRef<[u8]>,
{
    type Encoder = Utf8BytesEncoder;
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

impl<T> Encode for ResourceOwn<T>
where
    T: ?Sized,
{
    type Encoder = ResourceEncoder;
}

impl<T> Encode for &ResourceOwn<T>
where
    T: ?Sized,
{
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

impl<T> Encode for ResourceBorrow<T>
where
    T: ?Sized,
{
    type Encoder = ResourceEncoder;
}

impl<T> Encode for &ResourceBorrow<T>
where
    T: ?Sized,
{
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

impl<T: ?Sized> Receive for ResourceBorrowDecoder<T> {
    sync_receive!();
}

impl<T: ?Sized> Receive for CoreVecDecoder<ResourceBorrowDecoder<T>> {
    sync_receive!();
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

impl<T: ?Sized> Receive for ResourceOwnDecoder<T> {
    sync_receive!();
}

impl<T: ?Sized> Receive for CoreVecDecoder<ResourceOwnDecoder<T>> {
    sync_receive!();
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

impl Encode for () {
    type Encoder = UnitCodec;
}

impl Decode for () {
    type Decoder = UnitCodec;
    type ListDecoder = CoreVecDecoder<Self::Decoder>;
}

macro_rules! impl_tuple_codec {
    (@impl $(($vt:ident $ct:ident $cn:ident $i:tt))+) => {
        impl<$($ct: Transmit),+> Transmit for TupleEncoder::<($($ct),+,)> {
            fn poll_transmit(
                &mut self,
                cx: &mut Context<'_>,
                path: &mut Vec<usize>,
                dst: &mut BytesMut,
            ) -> Poll<std::io::Result<()>> {
                let ($($cn),+,) = &mut self.0;
                let mut ready = true;
                $(
                    path.push($i);
                    let res = $cn.poll_transmit(cx, path, dst);
                    path.pop();
                    match res {
                        Poll::Ready(Ok(())) => {}
                        Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                        Poll::Pending => ready = false,
                    }
                )+
                if ready {
                    Poll::Ready(Ok(()))
                } else {
                    Poll::Pending
                }
            }

            fn is_closed(&self) -> bool {
                let ($($cn),+,) = &self.0;
                true $(&& $cn.is_closed())+
            }
        }

        impl<$($vt),+> Encode for ($($vt),+,)
        where
            $(
                $vt: Encode,
            )+
        {
            type Encoder = TupleEncoder::<($($vt::Encoder),+,)>;
        }

        impl<'a, $($vt),+> Encode for &'a ($($vt),+,)
        where
            $(
                $vt: Encode,
                $vt::Encoder: tokio_util::codec::Encoder<&'a $vt, Error = std::io::Error>,
            )+
        {
            type Encoder = TupleEncoder::<($($vt::Encoder),+,)>;
        }

        impl<$($ct: Receive),+, V> Receive for TupleDecoder<($($ct),+,), V> {
            fn poll_receive(
                &mut self,
                cx: &mut Context<'_>,
                path: &[usize],
                src: &mut BytesMut,
            ) -> Poll<std::io::Result<()>> {
                let Some((i, rest)) = path.split_first() else {
                    return Poll::Ready(Err(error_unknown_path(path)));
                };
                let ($($cn),+,) = self.get_mut();
                match i {
                    $($i => $cn.poll_receive(cx, rest, src),)+
                    _ => Poll::Ready(Err(error_unknown_path(path))),
                }
            }

            fn is_closed(&self) -> bool {
                let ($($cn),+,) = self.get_ref();
                true $(&& $cn.is_closed())+
            }
        }

        impl<$($vt),+> Decode for ($($vt),+,)
        where
            $(
                $vt: Decode + Send + 'static,
            )+
        {
            type Decoder = TupleDecoder::<($($vt::Decoder),+,), ($(Option<$vt>),+,)>;
            type ListDecoder = ListDecoder<Self::Decoder>;
        }
    };
    ([$($acc:tt)*]) => {};
    ([$($acc:tt)*] $next:tt $($rest:tt)*) => {
        impl_tuple_codec!(@impl $($acc)* $next);
        impl_tuple_codec!([$($acc)* $next] $($rest)*);
    };
}

impl_tuple_codec!(
    []
    (V0 C0 c0 0)
    (V1 C1 c1 1)
    (V2 C2 c2 2)
    (V3 C3 c3 3)
    (V4 C4 c4 4)
    (V5 C5 c5 5)
    (V6 C6 c6 6)
    (V7 C7 c7 7)
    (V8 C8 c8 8)
    (V9 C9 c9 9)
    (V10 C10 c10 10)
    (V11 C11 c11 11)
    (V12 C12 c12 12)
    (V13 C13 c13 13)
    (V14 C14 c14 14)
    (V15 C15 c15 15)
);

/// Encoder for `future<T>`, whose value is transmitted on its path and the values nested in it
/// below `0`
pub struct FutureEncoder<T: Encode> {
    fut: Option<Pin<Box<dyn Future<Output = std::io::Result<T>> + Send>>>,
    nested: T::Encoder,
}

impl<T: Encode> Default for FutureEncoder<T> {
    fn default() -> Self {
        Self {
            fut: None,
            nested: T::Encoder::default(),
        }
    }
}

impl<T: Encode> FutureEncoder<T> {
    /// Returns the encoder of the pending `fut`, whose encoding was written by the caller, failing
    /// the transmission if it fails
    pub fn pending(fut: impl Future<Output = std::io::Result<T>> + Send + 'static) -> Self {
        Self {
            fut: Some(Box::pin(fut)),
            nested: T::Encoder::default(),
        }
    }
}

impl<T> Transmit for FutureEncoder<T>
where
    T: Encode,
{
    fn poll_transmit(
        &mut self,
        cx: &mut Context<'_>,
        path: &mut Vec<usize>,
        dst: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        if let Some(fut) = &mut self.fut {
            let v = ready!(fut.as_mut().poll(cx))?;
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

    fn is_closed(&self) -> bool {
        self.fut.is_none() && self.nested.is_closed()
    }
}

impl<T, Fut> tokio_util::codec::Encoder<Fut> for FutureEncoder<T>
where
    T: Encode + 'static,
    Fut: Future<Output = T> + Send + Unpin + 'static,
{
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self, item), fields(ty = "future"))]
    fn encode(&mut self, mut item: Fut, dst: &mut BytesMut) -> std::io::Result<()> {
        if let Some(item) = (&mut item).now_or_never() {
            dst.reserve(1);
            dst.put_u8(0x01);
            self.fut = None;
            self.nested.encode(item, dst)?;
            return Ok(());
        }
        dst.reserve(1);
        dst.put_u8(0x00);
        self.fut = Some(Box::pin(item.map(Ok)));
        Ok(())
    }
}

impl<T> Encode for Pin<Box<dyn Future<Output = T> + Send>>
where
    T: Encode + 'static,
{
    type Encoder = FutureEncoder<T>;
}

/// Decoder for `future<T>`, whose value is received on its path and the values nested in it
/// below `0`
pub struct FutureDecoder<T>
where
    T: Decode,
{
    dec: OptionDecoder<T::Decoder>,
    /// Sender of the value of a pending future, until it is received
    tx: Option<oneshot::Sender<T>>,
    buf: BytesMut,
    closed: bool,
}

impl<T> Default for FutureDecoder<T>
where
    T: Decode,
{
    fn default() -> Self {
        Self {
            dec: OptionDecoder::default(),
            tx: None,
            buf: BytesMut::default(),
            closed: true,
        }
    }
}

impl<T> FutureDecoder<T>
where
    T: Decode + Send + 'static,
{
    /// Returns the decoder of a pending `future<T>`, whose encoding was read by the caller, along
    /// with a future, which resolves to its value once it is received
    pub fn pending() -> (Self, impl Future<Output = T> + Send + 'static) {
        let (tx, rx) = oneshot::channel();
        let dec = Self {
            dec: OptionDecoder::default(),
            tx: Some(tx),
            buf: BytesMut::default(),
            closed: false,
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

impl<T> Receive for FutureDecoder<T>
where
    T: Decode,
{
    fn poll_receive(
        &mut self,
        cx: &mut Context<'_>,
        path: &[usize],
        src: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        if let Some((i, rest)) = path.split_first() {
            if *i != 0 || self.tx.is_some() {
                return Poll::Ready(Err(error_unknown_path(path)));
            }
            return self.dec.poll_receive(cx, rest, src);
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
            self.dec.get_mut().decode_eof(&mut self.buf)
        } else {
            self.buf.unsplit(src.split());
            self.dec.get_mut().decode(&mut self.buf)
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
        self.closed && self.dec.is_closed()
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
            return Ok(Some(Box::pin(async { item })));
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

/// Encoder for `stream<T>`, whose chunks are transmitted on its path and the values nested in
/// its elements below their indices
pub struct StreamEncoder<T: Encode> {
    items: Option<Pin<Box<dyn Stream<Item = Vec<T>> + Send>>>,
    /// Index of the next element
    offset: usize,
    nested: BTreeMap<usize, T::Encoder>,
}

impl<T: Encode> Default for StreamEncoder<T> {
    fn default() -> Self {
        Self {
            items: None,
            offset: 0,
            nested: BTreeMap::default(),
        }
    }
}

impl<T: Encode> StreamEncoder<T> {
    /// Returns the encoder of the pending `items`, whose encoding was written by the caller
    pub fn pending(items: impl Stream<Item = Vec<T>> + Send + 'static) -> Self {
        Self {
            items: Some(Box::pin(items)),
            offset: 0,
            nested: BTreeMap::default(),
        }
    }
}

impl<T> Transmit for StreamEncoder<T>
where
    T: Encode,
{
    fn poll_transmit(
        &mut self,
        cx: &mut Context<'_>,
        path: &mut Vec<usize>,
        dst: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        while let Some(items) = &mut self.items {
            match items.as_mut().poll_next(cx) {
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

    fn is_closed(&self) -> bool {
        self.items.is_none() && self.nested.is_closed()
    }
}

impl<T, S> tokio_util::codec::Encoder<S> for StreamEncoder<T>
where
    T: Encode + Send + 'static,
    S: Stream<Item = Vec<T>> + Send + Unpin + 'static,
{
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self, items), fields(ty = "stream"))]
    fn encode(&mut self, mut items: S, dst: &mut BytesMut) -> std::io::Result<()> {
        let mut ready = Vec::default();
        loop {
            match items.next().now_or_never() {
                Some(Some(chunk)) if ready.is_empty() => ready = chunk,
                Some(Some(chunk)) => ready.extend(chunk),
                Some(None) => {
                    dst.reserve(1);
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
        dst.reserve(1);
        dst.put_u8(0x00);
        *self = Self::pending(stream::iter([ready]).chain(items));
        Ok(())
    }
}

impl<T> Encode for Pin<Box<dyn Stream<Item = Vec<T>> + Send>>
where
    T: Encode + Send + 'static,
{
    type Encoder = StreamEncoder<T>;
}

/// Encoder for `stream<list<u8>>`, whose chunks are transmitted on its path
#[derive(Default)]
pub struct StreamEncoderBytes {
    items: Option<Pin<Box<dyn Stream<Item = Bytes> + Send>>>,
}

impl Transmit for StreamEncoderBytes {
    fn poll_transmit(
        &mut self,
        cx: &mut Context<'_>,
        path: &mut Vec<usize>,
        dst: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        while let Some(items) = &mut self.items {
            match ready!(items.as_mut().poll_next(cx)) {
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

    fn is_closed(&self) -> bool {
        self.items.is_none()
    }
}

impl<S> tokio_util::codec::Encoder<S> for StreamEncoderBytes
where
    S: Stream<Item = Bytes> + Send + Unpin + 'static,
{
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self, items), fields(ty = "stream<u8>"))]
    fn encode(&mut self, mut items: S, dst: &mut BytesMut) -> std::io::Result<()> {
        self.items = None;
        let mut ready = Vec::default();
        loop {
            match items.next().now_or_never() {
                Some(Some(chunk)) => ready.push(chunk),
                Some(None) => return encode_ready_bytes(&ready, dst),
                None => break,
            }
        }
        dst.reserve(1);
        dst.put_u8(0x00);
        self.items = Some(Box::pin(stream::iter(ready).chain(items)));
        Ok(())
    }
}

impl Encode for Pin<Box<dyn Stream<Item = Bytes> + Send>> {
    type Encoder = StreamEncoderBytes;
}

/// Encoder for `stream<list<u8>>` with [`AsyncRead`] support, whose chunks are transmitted on
/// its path
///
/// At most `MAX_READY` bytes are read from the stream on encode, any remaining
/// bytes are transmitted asynchronously.
#[derive(Default)]
pub struct StreamEncoderRead<const MAX_READY: usize = { usize::MAX }> {
    items: Option<Pin<Box<dyn AsyncRead + Send>>>,
    /// Data read, but not transmitted yet
    buf: BytesMut,
}

impl<const MAX_READY: usize> Transmit for StreamEncoderRead<MAX_READY> {
    fn poll_transmit(
        &mut self,
        cx: &mut Context<'_>,
        path: &mut Vec<usize>,
        dst: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        let Some(items) = &mut self.items else {
            return Poll::Ready(Ok(()));
        };
        if self.buf.is_empty() {
            self.buf.reserve(READ_CHUNK_SIZE);
            let n = ready!(poll_read_buf(
                items.as_mut(),
                cx,
                &mut (&mut self.buf).limit(READ_CHUNK_SIZE)
            ))?;
            if n == 0 {
                self.items = None;
                encode_close(path, dst)?;
                return Poll::Ready(Ok(()));
            }
        }
        encode_bytes_frames(path, &self.buf, dst)?;
        self.buf.clear();
        Poll::Pending
    }

    fn is_closed(&self) -> bool {
        self.items.is_none()
    }
}

impl<S, const MAX_READY: usize> tokio_util::codec::Encoder<S> for StreamEncoderRead<MAX_READY>
where
    S: AsyncRead + Send + Unpin + 'static,
{
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self, items), fields(ty = "stream<u8>"))]
    fn encode(&mut self, mut items: S, dst: &mut BytesMut) -> std::io::Result<()> {
        self.items = None;
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
        dst.reserve(1);
        dst.put_u8(0x00);
        self.items = Some(Box::pin(items));
        self.buf = ready;
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

fn poll_receive_stream<D, T>(
    cx: &mut Context<'_>,
    tx: &mut Option<(PollSender<T>, oneshot::Sender<()>)>,
    dec: &mut D,
    buf: &mut BytesMut,
    src: &mut BytesMut,
    decode: impl FnOnce(&mut D, &mut BytesMut) -> std::io::Result<Option<T>>,
) -> Poll<std::io::Result<()>>
where
    D: tokio_util::codec::Decoder<Error = std::io::Error>,
    T: Send,
{
    if src.is_empty() {
        let Some((.., end)) = tx.take() else {
            return Poll::Ready(Err(error_data_after_end()));
        };
        dec.decode_eof(buf)?;
        _ = end.send(());
        return Poll::Ready(Ok(()));
    }
    let Some((tx, ..)) = tx else {
        return Poll::Ready(Err(error_data_after_end()));
    };
    ready!(tx.poll_reserve(cx)).map_err(|_| error_receiver_closed())?;
    buf.unsplit(src.split());
    if let Some(chunk) = decode(dec, buf)? {
        tx.send_item(chunk).map_err(|_| error_receiver_closed())?;
    }
    Poll::Ready(Ok(()))
}

/// Decoder for `stream<T>`, whose chunks are received on its path and the values nested in its
/// elements below their indices
pub struct StreamDecoder<T>
where
    T: Decode,
{
    dec: OptionDecoder<ListDecoder<T::Decoder>>,
    /// Sender of the chunks and of the end of a pending stream, until it is closed
    tx: Option<(PollSender<Vec<T>>, oneshot::Sender<()>)>,
    buf: BytesMut,
    /// Index of the next element
    offset: usize,
    nested: BTreeMap<usize, T::Decoder>,
}

impl<T> Default for StreamDecoder<T>
where
    T: Decode,
{
    fn default() -> Self {
        Self {
            dec: OptionDecoder::default(),
            tx: None,
            buf: BytesMut::default(),
            offset: 0,
            nested: BTreeMap::default(),
        }
    }
}

impl<T> StreamDecoder<T>
where
    T: Decode + Send + 'static,
{
    /// Returns the decoder of a pending `stream<T>`, whose encoding was read by the caller, along
    /// with a [Stream] of its chunks
    pub fn pending() -> (Self, impl Stream<Item = Vec<T>> + Send + 'static) {
        let (tx, rx) = mpsc::channel(128);
        let (end_tx, end_rx) = oneshot::channel();
        let dec = Self {
            tx: Some((PollSender::new(tx), end_tx)),
            ..Self::default()
        };
        (dec, receive_pending_stream(rx, end_rx))
    }
}

impl<T> Receive for StreamDecoder<T>
where
    T: Decode + Send + 'static,
{
    fn poll_receive(
        &mut self,
        cx: &mut Context<'_>,
        path: &[usize],
        src: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        if !path.is_empty() {
            return self.nested.poll_receive(cx, path, src);
        }
        poll_receive_stream(
            cx,
            &mut self.tx,
            self.dec.get_mut(),
            &mut self.buf,
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
                    let nested = mem::take(&mut dec.nested)
                        .into_iter()
                        .map(|(i, dec)| (offset.saturating_add(i), dec));
                    self.nested.extend(nested);
                    chunk.extend(items);
                }
                Ok((!chunk.is_empty()).then_some(chunk))
            },
        )
    }

    fn is_closed(&self) -> bool {
        self.tx.is_none() && self.nested.is_closed()
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
            self.nested = mem::take(&mut self.dec.get_mut().nested);
            if chunk.is_empty() {
                return Ok(Some(Box::pin(stream::empty())));
            }
            return Ok(Some(Box::pin(stream::iter([chunk]))));
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

/// Decoder for `stream<list<u8>>`, whose chunks are received on its path
#[derive(Default)]
pub struct StreamDecoderBytes {
    dec: OptionDecoder<CoreVecDecoderBytes>,
    /// Sender of the chunks and of the end of a pending stream, until it is closed
    tx: Option<(PollSender<Bytes>, oneshot::Sender<()>)>,
    buf: BytesMut,
}

impl StreamDecoderBytes {
    /// Returns the decoder of a pending `stream<u8>`, whose encoding was read by the caller, along
    /// with a [Stream] of its chunks
    pub fn pending() -> (Self, impl Stream<Item = Bytes> + Send + 'static) {
        let (tx, rx) = mpsc::channel(128);
        let (end_tx, end_rx) = oneshot::channel();
        let dec = Self {
            tx: Some((PollSender::new(tx), end_tx)),
            ..Self::default()
        };
        (dec, receive_pending_stream(rx, end_rx))
    }
}

impl Receive for StreamDecoderBytes {
    fn poll_receive(
        &mut self,
        cx: &mut Context<'_>,
        path: &[usize],
        src: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        if !path.is_empty() {
            return Poll::Ready(Err(error_unknown_path(path)));
        }
        poll_receive_stream(
            cx,
            &mut self.tx,
            self.dec.get_mut(),
            &mut self.buf,
            src,
            |dec, buf| {
                let Some(mut chunk) = dec.decode(buf)? else {
                    return Ok(None);
                };
                while let Some(data) = dec.decode(buf)? {
                    chunk = [chunk, data].concat().into();
                }
                Ok((!chunk.is_empty()).then_some(chunk))
            },
        )
    }

    fn is_closed(&self) -> bool {
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
            if chunk.is_empty() {
                return Ok(Some(Box::pin(stream::empty())));
            }
            return Ok(Some(Box::pin(stream::iter([chunk]))));
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

/// Decoder for `stream<list<u8>>` with [`AsyncRead`] support
#[derive(Default)]
pub struct StreamDecoderRead {
    dec: StreamDecoderBytes,
}

impl Receive for StreamDecoderRead {
    fn poll_receive(
        &mut self,
        cx: &mut Context<'_>,
        path: &[usize],
        src: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        self.dec.poll_receive(cx, path, src)
    }

    fn is_closed(&self) -> bool {
        self.dec.is_closed()
    }
}

impl tokio_util::codec::Decoder for StreamDecoderRead {
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

impl Decode for Pin<Box<dyn AsyncRead + Send>> {
    type Decoder = StreamDecoderRead;
    type ListDecoder = ListDecoder<Self::Decoder>;
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use anyhow::bail;
    use futures::future::{BoxFuture, join_all};
    use futures::stream::BoxStream;
    use tokio::io::duplex;
    use tokio::select;
    use tokio::task::{spawn, yield_now};
    use tokio::time;
    use tokio_util::codec::{FramedRead, FramedWrite};

    use super::*;
    use crate::mux::{receive, transmit};
    use crate::{FrameDecoder, FrameEncoder, FrameRef};

    #[test_log::test(tokio::test)]
    async fn roundtrip() -> anyhow::Result<()> {
        type V = (
            [[u32; 2]; 2],
            Vec<BoxFuture<'static, u8>>,
            BoxFuture<'static, Vec<BoxFuture<'static, u8>>>,
            BoxFuture<'static, ()>,
            BoxStream<'static, Bytes>,
            Pin<Box<dyn AsyncRead + Send>>,
        );

        fn delay<T: Send + 'static>(v: T) -> BoxFuture<'static, T> {
            Box::pin(async move {
                yield_now().await;
                v
            })
        }

        let data: Bytes = (0..=u8::MAX)
            .cycle()
            .take(2 * READ_CHUNK_SIZE + 1)
            .collect();
        let v: V = (
            [[1, 300], [2, 3]],
            vec![delay(1)],
            delay(vec![delay(2)]),
            delay(()),
            Box::pin(stream::iter(["foo", "", "bar"].map(Bytes::from)).then(delay)),
            Box::pin(std::io::Cursor::new(data.clone())),
        );
        let (tx, rx) = duplex(1024);
        let tx = transmit(
            <V as Encode>::Encoder::default(),
            FramedWrite::new(tx, FrameEncoder),
            v,
        )?;
        let tx = spawn(tx);
        let ((a, b, c, d, e, mut f), rx) = receive(
            <V as Decode>::Decoder::default(),
            FramedRead::new(rx, FrameDecoder::default()),
        )
        .await?;
        let rx = spawn(rx);
        assert_eq!(a, [[1, 300], [2, 3]]);
        assert_eq!(join_all(b).await, [1]);
        assert_eq!(join_all(c.await).await, [2]);
        d.await;
        let e: Vec<Bytes> = e.collect().await;
        assert_eq!(e.concat(), b"foobar");
        let mut buf = Vec::default();
        f.read_to_end(&mut buf).await?;
        assert_eq!(buf, data);
        tx.await??;
        rx.await??;
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn future_flushes_before_nested_value() -> anyhow::Result<()> {
        type V = (BoxFuture<'static, BoxFuture<'static, u8>>,);

        let outer: BoxFuture<'static, BoxFuture<'static, u8>> =
            Box::pin(yield_now().map(|()| Box::pin(pending()) as BoxFuture<'static, u8>));
        let (tx, rx) = duplex(1024);
        let tx = transmit(
            <V as Encode>::Encoder::default(),
            FramedWrite::new(tx, FrameEncoder),
            (outer,),
        )?;
        let rx = async {
            let ((outer,), rx) = receive(
                <V as Decode>::Decoder::default(),
                FramedRead::new(rx, FrameDecoder::default()),
            )
            .await?;
            select! {
                res = rx => bail!("nested value received: {res:?}"),
                _ = outer => Ok(()),
            }
        };
        select! {
            res = tx => bail!("nested value transmitted: {res:?}"),
            res = time::timeout(Duration::from_secs(5), rx) => res.expect("value was not flushed"),
        }
    }

    #[test_log::test(tokio::test)]
    async fn ingress_fails_fast() -> anyhow::Result<()> {
        async fn receive_value<T>(
            data: &[u8],
        ) -> std::io::Result<(T, impl Future<Output = std::io::Result<()>> + use<T>)>
        where
            (T,): Decode,
        {
            let mut buf = BytesMut::default();
            let frames: [(&[usize], &[u8]); 4] =
                [(&[], b"\0"), (&[], b""), (&[0], data), (&[0], b"")];
            for (path, data) in frames {
                FrameEncoder.encode(FrameRef { path, data }, &mut buf)?;
            }
            let rx = FramedRead::new(std::io::Cursor::new(buf), FrameDecoder::default());
            let dec = <(T,) as Decode>::Decoder::default();
            let Ok(((v,), rx)) = receive(dec, rx).await else {
                panic!("value not decoded")
            };
            Ok((v, rx))
        }

        let (_fut, rx) = receive_value::<BoxFuture<'static, ()>>(b"\x2a").await?;
        let err = rx.await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

        let (_fut, rx) = receive_value::<BoxFuture<'static, Bytes>>(b"\x03foo\x2a").await?;
        let err = rx.await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

        let (fut, rx) = receive_value::<BoxFuture<'static, Bytes>>(b"\x03foo").await?;
        drop(fut);
        let err = rx.await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);
        Ok(())
    }

    #[test]
    fn list_length_exceeds_data() {
        const LEN: &[u8] = b"\xff\xff\xff\xff\x0f";

        let err = crate::unpack(
            <Vec<String> as Decode>::Decoder::default(),
            &mut BytesMut::from(LEN),
        )
        .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
        let err = crate::unpack(
            <Bytes as Decode>::Decoder::default(),
            &mut BytesMut::from(LEN),
        )
        .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
    }
}
