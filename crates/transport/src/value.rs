use core::any;
use core::fmt::{self, Debug};
use core::future::{self, pending};
use core::hash::{BuildHasher, Hash, Hasher};
use core::marker::PhantomData;
use core::mem;
use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use core::str;
use core::task::{Context, Poll, ready};

use std::collections::{BTreeMap, HashMap};
use std::vec;

use bytes::{Buf, BufMut as _, Bytes, BytesMut};
use futures::channel::mpsc;
use futures::stream;
use futures::{FutureExt as _, Stream, StreamExt as _};
use tokio::io::{AsyncRead, AsyncReadExt as _};
use tokio::sync::oneshot;
use tokio_util::codec::{Decoder as _, Encoder as _};
use tokio_util::io::{StreamReader, poll_read_buf};
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
const READ_CHUNK_SIZE: usize = 8192;

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

macro_rules! impl_resource {
    ($(#[$attr:meta])* $name:ident, $kind:literal) => {
        $(#[$attr])*
        #[repr(transparent)]
        pub struct $name<T: ?Sized> {
            repr: Bytes,
            _ty: PhantomData<T>,
        }

        impl<T: ?Sized> From<Bytes> for $name<T> {
            fn from(repr: Bytes) -> Self {
                Self {
                    repr,
                    _ty: PhantomData,
                }
            }
        }

        impl<T: ?Sized> From<Vec<u8>> for $name<T> {
            fn from(repr: Vec<u8>) -> Self {
                Self::from(Bytes::from(repr))
            }
        }

        impl<T: ?Sized> From<$name<T>> for Bytes {
            fn from($name { repr, .. }: $name<T>) -> Self {
                repr
            }
        }

        impl<T: ?Sized> PartialEq for $name<T> {
            fn eq(&self, other: &Self) -> bool {
                self.repr == other.repr
            }
        }

        impl<T: ?Sized> Eq for $name<T> {}

        impl<T: ?Sized> Hash for $name<T> {
            fn hash<H: Hasher>(&self, state: &mut H) {
                self.repr.hash(state);
            }
        }

        impl<T: ?Sized> AsRef<[u8]> for $name<T> {
            fn as_ref(&self) -> &[u8] {
                &self.repr
            }
        }

        impl<T: ?Sized> AsRef<Bytes> for $name<T> {
            fn as_ref(&self) -> &Bytes {
                &self.repr
            }
        }

        impl<T: ?Sized> Debug for $name<T> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}<{}>", $kind, any::type_name::<T>())
            }
        }

        impl<T: ?Sized> Clone for $name<T> {
            fn clone(&self) -> Self {
                Self::from(self.repr.clone())
            }
        }

        impl<T: ?Sized> $name<T> {
            /// Constructs a new resource handle
            pub fn new(repr: impl Into<Bytes>) -> Self {
                Self::from(repr.into())
            }
        }
    };
}

impl_resource!(
    /// Borrowed resource handle, represented as an opaque byte blob
    ResourceBorrow,
    "borrow"
);

impl_resource!(
    /// Owned resource handle, represented as an opaque byte blob
    ResourceOwn,
    "own"
);

impl<T: ?Sized> From<ResourceOwn<T>> for ResourceBorrow<T> {
    fn from(ResourceOwn { repr, .. }: ResourceOwn<T>) -> Self {
        Self::from(repr)
    }
}

impl<T: ?Sized> ResourceOwn<T> {
    /// Returns the owned handle as [`ResourceBorrow`]
    pub fn as_borrow(&self) -> ResourceBorrow<T> {
        ResourceBorrow::from(self.repr.clone())
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

    /// Returns whether nothing in the last encoded value is pending
    fn is_complete(&self) -> bool;
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

    fn is_complete(&self) -> bool {
        (**self).is_complete()
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

    fn is_complete(&self) -> bool {
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

    /// Returns whether nothing in the last decoded value is pending
    fn is_complete(&self) -> bool;
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

    fn is_complete(&self) -> bool {
        (**self).is_complete()
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
        if r.is_complete() {
            self.remove(i);
        }
        Poll::Ready(Ok(()))
    }

    fn is_complete(&self) -> bool {
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

        fn is_complete(&self) -> bool {
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

        fn is_complete(&self) -> bool {
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
    type Encoder: tokio_util::codec::Encoder<Self, Error = std::io::Error> + Transmit + Default;

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
        encode_items(items.into_iter(), dst)
    }
}

fn encode_items<E, I>(items: I, dst: &mut BytesMut) -> std::io::Result<BTreeMap<usize, E>>
where
    E: tokio_util::codec::Encoder<I::Item, Error = std::io::Error> + Transmit + Default,
    I: ExactSizeIterator,
{
    dst.reserve(items.len());
    let mut enc = E::default();
    let mut nested = BTreeMap::default();
    for (i, item) in items.enumerate() {
        enc.encode(item, dst)?;
        if !enc.is_complete() {
            nested.insert(i, mem::take(&mut enc));
        }
    }
    Ok(nested)
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
        + 'static;
    /// Decoder used to decode lists of value
    type ListDecoder: tokio_util::codec::Decoder<Item = Vec<Self>, Error = std::io::Error>
        + Default
        + 'static;

    #[doc(hidden)]
    fn decode_chunk(
        dec: &mut ListDecoder<Self::Decoder>,
        src: &mut BytesMut,
    ) -> std::io::Result<Option<Vec<Self>>> {
        dec.decode(src)
    }
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

    fn is_complete(&self) -> bool {
        self.0.is_complete()
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

    fn is_complete(&self) -> bool {
        self.get_ref().is_complete()
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

    fn is_complete(&self) -> bool {
        self.ok.is_complete() && self.err.is_complete()
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
        if ok.is_complete() {
            err.poll_receive(cx, path, src)
        } else {
            ok.poll_receive(cx, path, src)
        }
    }

    fn is_complete(&self) -> bool {
        let (ok, err) = self.get_ref();
        ok.is_complete() && err.is_complete()
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

/// Encoder for `list<T>`, `list<T, N>` and `map<K, V>`
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

    fn is_complete(&self) -> bool {
        self.nested.is_complete()
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

    fn is_complete(&self) -> bool {
        self.nested.is_complete()
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
                len.min(src.len())
                    .min(DEFAULT_MAX_INITIAL_CAPACITY / mem::size_of::<T::Item>().max(1)),
            );
            self.cap = len;
        }
        while self.cap > 0 {
            let Some(v) = self.dec.decode(src)? else {
                return Ok(None);
            };
            if !self.dec.is_complete() {
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
    T: Decode,
    T::ListDecoder: Receive,
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

    fn is_complete(&self) -> bool {
        self.nested.is_complete()
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
            if !self.dec.is_complete() {
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
    T: Decode,
{
    type Decoder = ArrayDecoder<T::Decoder, N>;
    type ListDecoder = ListDecoder<Self::Decoder>;
}

#[instrument(level = "trace", skip(items), fields(ty = "map"))]
fn encode_map<E, I>(items: I, dst: &mut BytesMut) -> std::io::Result<BTreeMap<usize, E>>
where
    E: tokio_util::codec::Encoder<I::Item, Error = std::io::Error> + Transmit + Default,
    I: IntoIterator,
    I::IntoIter: ExactSizeIterator,
{
    let items = items.into_iter();
    let n = u32::try_from(items.len())
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
    dst.reserve(5);
    Leb128Encoder.encode(n, dst)?;
    encode_items(items, dst)
}

impl<K, V> tokio_util::codec::Encoder<BTreeMap<K, V>>
    for ListEncoder<TupleEncoder<(K::Encoder, V::Encoder)>>
where
    K: Encode,
    V: Encode,
{
    type Error = std::io::Error;

    fn encode(&mut self, items: BTreeMap<K, V>, dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.nested = encode_map(items, dst)?;
        Ok(())
    }
}

impl<'a, K, V> tokio_util::codec::Encoder<&'a BTreeMap<K, V>>
    for ListEncoder<TupleEncoder<(K::Encoder, V::Encoder)>>
where
    K: Encode,
    V: Encode,
    K::Encoder: tokio_util::codec::Encoder<&'a K, Error = std::io::Error>,
    V::Encoder: tokio_util::codec::Encoder<&'a V, Error = std::io::Error>,
{
    type Error = std::io::Error;

    fn encode(&mut self, items: &'a BTreeMap<K, V>, dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.nested = encode_map(items, dst)?;
        Ok(())
    }
}

impl<'a, 'b, K, V> tokio_util::codec::Encoder<&'a &'b BTreeMap<K, V>>
    for ListEncoder<TupleEncoder<(K::Encoder, V::Encoder)>>
where
    K: Encode,
    V: Encode,
    K::Encoder: tokio_util::codec::Encoder<&'b K, Error = std::io::Error>,
    V::Encoder: tokio_util::codec::Encoder<&'b V, Error = std::io::Error>,
{
    type Error = std::io::Error;

    fn encode(
        &mut self,
        items: &'a &'b BTreeMap<K, V>,
        dst: &mut BytesMut,
    ) -> Result<(), Self::Error> {
        self.nested = encode_map(*items, dst)?;
        Ok(())
    }
}

impl<K, V, S> tokio_util::codec::Encoder<HashMap<K, V, S>>
    for ListEncoder<TupleEncoder<(K::Encoder, V::Encoder)>>
where
    K: Encode,
    V: Encode,
{
    type Error = std::io::Error;

    fn encode(&mut self, items: HashMap<K, V, S>, dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.nested = encode_map(items, dst)?;
        Ok(())
    }
}

impl<'a, K, V, S> tokio_util::codec::Encoder<&'a HashMap<K, V, S>>
    for ListEncoder<TupleEncoder<(K::Encoder, V::Encoder)>>
where
    K: Encode,
    V: Encode,
    K::Encoder: tokio_util::codec::Encoder<&'a K, Error = std::io::Error>,
    V::Encoder: tokio_util::codec::Encoder<&'a V, Error = std::io::Error>,
{
    type Error = std::io::Error;

    fn encode(
        &mut self,
        items: &'a HashMap<K, V, S>,
        dst: &mut BytesMut,
    ) -> Result<(), Self::Error> {
        self.nested = encode_map(items, dst)?;
        Ok(())
    }
}

impl<'a, 'b, K, V, S> tokio_util::codec::Encoder<&'a &'b HashMap<K, V, S>>
    for ListEncoder<TupleEncoder<(K::Encoder, V::Encoder)>>
where
    K: Encode,
    V: Encode,
    K::Encoder: tokio_util::codec::Encoder<&'b K, Error = std::io::Error>,
    V::Encoder: tokio_util::codec::Encoder<&'b V, Error = std::io::Error>,
{
    type Error = std::io::Error;

    fn encode(
        &mut self,
        items: &'a &'b HashMap<K, V, S>,
        dst: &mut BytesMut,
    ) -> Result<(), Self::Error> {
        self.nested = encode_map(*items, dst)?;
        Ok(())
    }
}

impl<K, V> Encode for BTreeMap<K, V>
where
    K: Encode,
    V: Encode,
{
    type Encoder = ListEncoder<TupleEncoder<(K::Encoder, V::Encoder)>>;
}

impl<'a, K, V> Encode for &'a BTreeMap<K, V>
where
    K: Encode,
    V: Encode,
    K::Encoder: tokio_util::codec::Encoder<&'a K, Error = std::io::Error>,
    V::Encoder: tokio_util::codec::Encoder<&'a V, Error = std::io::Error>,
{
    type Encoder = ListEncoder<TupleEncoder<(K::Encoder, V::Encoder)>>;
}

impl<K, V, S> Encode for HashMap<K, V, S>
where
    K: Encode,
    V: Encode,
{
    type Encoder = ListEncoder<TupleEncoder<(K::Encoder, V::Encoder)>>;
}

impl<'a, K, V, S> Encode for &'a HashMap<K, V, S>
where
    K: Encode,
    V: Encode,
    K::Encoder: tokio_util::codec::Encoder<&'a K, Error = std::io::Error>,
    V::Encoder: tokio_util::codec::Encoder<&'a V, Error = std::io::Error>,
{
    type Encoder = ListEncoder<TupleEncoder<(K::Encoder, V::Encoder)>>;
}

/// Decoder for `map<K, V>`, which collects the entries decoded by `T` into `M`
pub struct MapDecoder<T, M>
where
    T: tokio_util::codec::Decoder,
{
    list: ListDecoder<T>,
    _ty: PhantomData<fn() -> M>,
}

impl<T, M> Default for MapDecoder<T, M>
where
    T: tokio_util::codec::Decoder + Default,
{
    fn default() -> Self {
        Self {
            list: ListDecoder::default(),
            _ty: PhantomData,
        }
    }
}

impl<T, M> Receive for MapDecoder<T, M>
where
    T: tokio_util::codec::Decoder + Receive,
{
    fn poll_receive(
        &mut self,
        cx: &mut Context<'_>,
        path: &[usize],
        src: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        self.list.poll_receive(cx, path, src)
    }

    fn is_complete(&self) -> bool {
        self.list.is_complete()
    }
}

fn collect_map<T, M>(items: Vec<T>) -> std::io::Result<M>
where
    M: FromIterator<T>,
    for<'a> &'a M: IntoIterator<IntoIter: ExactSizeIterator>,
{
    let n = items.len();
    let map: M = items.into_iter().collect();
    if (&map).into_iter().len() != n {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "map contains duplicate keys",
        ));
    }
    Ok(map)
}

impl<T, M> tokio_util::codec::Decoder for MapDecoder<T, M>
where
    T: tokio_util::codec::Decoder + Receive + Default,
    M: FromIterator<T::Item>,
    for<'a> &'a M: IntoIterator<IntoIter: ExactSizeIterator>,
{
    type Item = M;
    type Error = T::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let items = self.list.decode(src)?;
        items.map(collect_map).transpose().map_err(Into::into)
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let items = self.list.decode_eof(src)?;
        items.map(collect_map).transpose().map_err(Into::into)
    }
}

impl<K, V> Decode for BTreeMap<K, V>
where
    K: Decode + Ord + 'static,
    V: Decode + 'static,
{
    type Decoder = MapDecoder<<(K, V) as Decode>::Decoder, Self>;
    type ListDecoder = ListDecoder<Self::Decoder>;
}

impl<K, V, S> Decode for HashMap<K, V, S>
where
    K: Decode + Eq + Hash + 'static,
    V: Decode + 'static,
    S: BuildHasher + Default + 'static,
{
    type Decoder = MapDecoder<<(K, V) as Decode>::Decoder, Self>;
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
        let buf = self.0.decode(src)?;
        Ok(buf.map(Vec::from))
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let buf = self.0.decode_eof(src)?;
        Ok(buf.map(Vec::from))
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
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
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

impl<T: ?Sized> Encode for ResourceOwn<T> {
    type Encoder = CoreVecEncoderBytes;
}

impl<T: ?Sized> Encode for &ResourceOwn<T> {
    type Encoder = CoreVecEncoderBytes;
}

impl<T: ?Sized> Encode for ResourceBorrow<T> {
    type Encoder = CoreVecEncoderBytes;
}

impl<T: ?Sized> Encode for &ResourceBorrow<T> {
    type Encoder = CoreVecEncoderBytes;
}

/// Decoder for resource handles of type `T`
#[derive(Debug)]
pub struct ResourceDecoder<T> {
    dec: CoreVecDecoderBytes,
    _ty: PhantomData<fn() -> T>,
}

impl<T> Default for ResourceDecoder<T> {
    fn default() -> Self {
        Self {
            dec: CoreVecDecoderBytes::default(),
            _ty: PhantomData,
        }
    }
}

impl<T> Receive for ResourceDecoder<T> {
    sync_receive!();
}

impl<T: From<Bytes>> Receive for CoreVecDecoder<ResourceDecoder<T>> {
    sync_receive!();
}

impl<T: From<Bytes>> tokio_util::codec::Decoder for ResourceDecoder<T> {
    type Item = T;
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self), fields(ty = "resource"))]
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let repr = self.dec.decode(src)?;
        Ok(repr.map(T::from))
    }
}

impl<T: ?Sized + 'static> Decode for ResourceOwn<T> {
    type Decoder = ResourceDecoder<Self>;
    type ListDecoder = CoreVecDecoder<Self::Decoder>;
}

impl<T: ?Sized + 'static> Decode for ResourceBorrow<T> {
    type Decoder = ResourceDecoder<Self>;
    type ListDecoder = CoreVecDecoder<Self::Decoder>;
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

    fn decode_chunk(
        _: &mut ListDecoder<Self::Decoder>,
        src: &mut BytesMut,
    ) -> std::io::Result<Option<Vec<Self>>> {
        let Some(len) = Leb128DecoderU32.decode(src)? else {
            return Ok(None);
        };
        let len = usize::try_from(len)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        Ok(Some(vec![(); len]))
    }
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
                    let res = res?;
                    ready &= res.is_ready();
                )+
                if ready {
                    Poll::Ready(Ok(()))
                } else {
                    Poll::Pending
                }
            }

            fn is_complete(&self) -> bool {
                let ($($cn),+,) = &self.0;
                true $(&& $cn.is_complete())+
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

            fn is_complete(&self) -> bool {
                let ($($cn),+,) = self.get_ref();
                true $(&& $cn.is_complete())+
            }
        }

        impl<$($vt),+> Decode for ($($vt),+,)
        where
            $(
                $vt: Decode + 'static,
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
);

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
            self.dec.get_mut().decode_eof(&mut self.buffer)
        } else {
            let dec = self.dec.get_mut();
            decode_buffered(&mut self.buffer, src, |buf| dec.decode(buf))
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
        if !self.buffer.is_empty() {
            return Poll::Ready(Err(error_data_after_end()));
        }
        if tx.send(v).is_err() {
            return Poll::Ready(Err(error_receiver_closed()));
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

/// Encoder for `stream<list<u8>>` of type `T`, whose chunks are transmitted on its path
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

/// Encoder for `stream<list<u8>>` with [`AsyncRead`] support of type `T`, whose chunks are
/// transmitted on its path
pub struct StreamEncoderRead<T> {
    items: Option<T>,
    /// Data read, but not transmitted yet
    buffer: BytesMut,
}

impl<T> Default for StreamEncoderRead<T> {
    fn default() -> Self {
        Self {
            items: None,
            buffer: BytesMut::default(),
        }
    }
}

impl<T> Transmit for StreamEncoderRead<T>
where
    T: AsyncRead + Unpin,
{
    fn poll_transmit(
        &mut self,
        cx: &mut Context<'_>,
        path: &mut Vec<usize>,
        dst: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        let Some(items) = &mut self.items else {
            return Poll::Ready(Ok(()));
        };
        if self.buffer.is_empty() {
            self.buffer.reserve(READ_CHUNK_SIZE);
            let n = ready!(poll_read_buf(
                Pin::new(items),
                cx,
                &mut (&mut self.buffer).limit(READ_CHUNK_SIZE)
            ))?;
            if n == 0 {
                self.items = None;
                encode_close(path, dst)?;
                return Poll::Ready(Ok(()));
            }
        }
        encode_bytes_frames(path, &self.buffer, dst)?;
        self.buffer.clear();
        Poll::Pending
    }

    fn is_complete(&self) -> bool {
        self.items.is_none()
    }
}

impl<T> tokio_util::codec::Encoder<T> for StreamEncoderRead<T>
where
    T: AsyncRead + Unpin,
{
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self, items), fields(ty = "stream<u8>"))]
    fn encode(&mut self, mut items: T, dst: &mut BytesMut) -> std::io::Result<()> {
        self.items = None;
        let mut ready = BytesMut::default();
        while ready.len() < MAX_READY {
            let n = MAX_READY.saturating_sub(ready.len()).min(READ_CHUNK_SIZE);
            ready.reserve(n);
            match items.read_buf(&mut (&mut ready).limit(n)).now_or_never() {
                Some(Ok(0)) => return encode_ready_bytes(&[ready], dst),
                Some(Ok(..)) => {}
                Some(Err(err)) => return Err(err),
                None => break,
            }
        }
        dst.put_u8(0x00);
        self.items = Some(items);
        self.buffer = ready;
        Ok(())
    }
}

impl Encode for Pin<Box<dyn AsyncRead + Send>> {
    type Encoder = StreamEncoderRead<Self>;
}

impl Encode for Pin<Box<dyn AsyncRead>> {
    type Encoder = StreamEncoderRead<Self>;
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

fn decode_buffered<T>(
    buf: &mut BytesMut,
    src: &mut BytesMut,
    decode: impl FnOnce(&mut BytesMut) -> std::io::Result<T>,
) -> std::io::Result<T> {
    if !buf.is_empty() {
        buf.extend_from_slice(src);
        src.clear();
        return decode(buf);
    }
    let v = decode(src)?;
    buf.extend_from_slice(src);
    src.clear();
    Ok(v)
}

fn poll_receive_stream<D, T>(
    cx: &mut Context<'_>,
    tx: &mut Option<(mpsc::Sender<T>, oneshot::Sender<()>)>,
    dec: &mut D,
    buf: &mut BytesMut,
    src: &mut BytesMut,
    decode: impl FnOnce(&mut D, &mut BytesMut) -> std::io::Result<Option<T>>,
) -> Poll<std::io::Result<()>>
where
    D: tokio_util::codec::Decoder<Error = std::io::Error>,
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
    ready!(tx.poll_ready(cx)).map_err(|_| error_receiver_closed())?;
    if let Some(chunk) = decode_buffered(buf, src, |buf| decode(dec, buf))? {
        tx.start_send(chunk).map_err(|_| error_receiver_closed())?;
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
    ) -> Poll<std::io::Result<()>> {
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

/// Decoder for `stream<list<u8>>` of type `T`, whose chunks are received on its path
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
    ) -> Poll<std::io::Result<()>> {
        if !path.is_empty() {
            return Poll::Ready(Err(error_unknown_path(path)));
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

/// Decoder for `stream<list<u8>>` with [`AsyncRead`] support of type `T`
pub struct StreamDecoderRead<T = Pin<Box<dyn AsyncRead + Send>>> {
    dec: StreamDecoderBytes,
    _items: PhantomData<fn() -> T>,
}

impl<T> Default for StreamDecoderRead<T> {
    fn default() -> Self {
        Self {
            dec: StreamDecoderBytes::default(),
            _items: PhantomData,
        }
    }
}

impl<T> Receive for StreamDecoderRead<T> {
    fn poll_receive(
        &mut self,
        cx: &mut Context<'_>,
        path: &[usize],
        src: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        self.dec.poll_receive(cx, path, src)
    }

    fn is_complete(&self) -> bool {
        self.dec.is_complete()
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

impl tokio_util::codec::Decoder for StreamDecoderRead<Pin<Box<dyn AsyncRead>>> {
    type Item = Pin<Box<dyn AsyncRead>>;
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

impl Decode for Pin<Box<dyn AsyncRead>> {
    type Decoder = StreamDecoderRead<Self>;
    type ListDecoder = ListDecoder<Self::Decoder>;
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use std::rc::Rc;

    use anyhow::bail;
    use futures::future::{BoxFuture, LocalBoxFuture, join_all};
    use futures::stream::{BoxStream, LocalBoxStream};
    use tokio::task::{spawn, yield_now};
    use tokio::time;
    use tokio::{join, select, try_join};
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
            BTreeMap<String, BoxFuture<'static, u8>>,
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
            BTreeMap::from([("a".into(), delay(4)), ("b".into(), delay(5))]),
        );
        let (tx, rx) = tokio::io::duplex(1024);
        let tx = transmit(
            <V as Encode>::Encoder::default(),
            FramedWrite::new(tx, FrameEncoder),
            v,
        )?;
        let tx = spawn(tx);
        let ((a, b, c, d, e, mut f, g), rx) = receive(
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
        let (keys, values): (Vec<_>, Vec<_>) = g.into_iter().unzip();
        assert_eq!(keys, ["a", "b"]);
        assert_eq!(join_all(values).await, [4, 5]);
        tx.await??;
        rx.await??;
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn local_roundtrip() -> anyhow::Result<()> {
        type V = (
            LocalBoxFuture<'static, u8>,
            LocalBoxStream<'static, Vec<LocalBoxFuture<'static, u8>>>,
            LocalBoxStream<'static, Bytes>,
            Pin<Box<dyn AsyncRead>>,
        );

        fn delay(v: Rc<u8>) -> LocalBoxFuture<'static, u8> {
            Box::pin(async move {
                yield_now().await;
                *v
            })
        }

        let v: V = (
            delay(Rc::new(1)),
            Box::pin(stream::iter([2, 3]).then(async |v| {
                yield_now().await;
                vec![delay(Rc::new(v))]
            })),
            Box::pin(stream::iter([Rc::new(Bytes::from("foo"))]).then(async |v| {
                yield_now().await;
                Bytes::clone(&v)
            })),
            Box::pin(std::io::Cursor::new(Rc::<[u8]>::from(&b"bar"[..]))),
        );
        let (tx, rx) = tokio::io::duplex(1024);
        let tx = transmit(
            <V as Encode>::Encoder::default(),
            FramedWrite::new(tx, FrameEncoder),
            v,
        )?;
        let rx = async {
            let ((a, b, c, mut d), rx) = receive(
                <V as Decode>::Decoder::default(),
                FramedRead::new(rx, FrameDecoder::default()),
            )
            .await?;
            let (v, ()) = try_join!(
                async {
                    let mut buf = Vec::default();
                    let (a, b, c, n) = join!(
                        a,
                        b.concat(),
                        c.collect::<Vec<Bytes>>(),
                        d.read_to_end(&mut buf)
                    );
                    n?;
                    Ok((a, join_all(b).await, c.concat(), buf))
                },
                rx
            )?;
            std::io::Result::Ok(v)
        };
        let ((), (a, b, c, d)) = try_join!(tx, rx)?;
        assert_eq!(a, 1);
        assert_eq!(b, [2, 3]);
        assert_eq!(c, b"foo");
        assert_eq!(d, b"bar");
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn future_flushes_before_nested_value() -> anyhow::Result<()> {
        type V = (BoxFuture<'static, BoxFuture<'static, u8>>,);

        let outer: BoxFuture<'static, BoxFuture<'static, u8>> =
            Box::pin(yield_now().map(|()| Box::pin(pending()) as BoxFuture<'static, u8>));
        let (tx, rx) = tokio::io::duplex(1024);
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
    fn map_encodes_as_list() -> std::io::Result<()> {
        let v: HashMap<String, u32> = HashMap::from([("a".into(), 1)]);
        let mut buf = BytesMut::default();
        crate::pack(
            <HashMap<String, u32> as Encode>::Encoder::default(),
            v,
            &mut buf,
        )?;
        assert_eq!(buf, b"\x01\x01a\x01"[..]);
        let v = crate::unpack(
            <BTreeMap<String, u32> as Decode>::Decoder::default(),
            &mut buf,
        )?;
        assert_eq!(v, BTreeMap::from([("a".into(), 1)]));
        Ok(())
    }

    #[test]
    fn map_rejects_duplicate_keys() {
        let err = crate::unpack(
            <HashMap<String, u32> as Decode>::Decoder::default(),
            &mut BytesMut::from(&b"\x02\x01a\x01\x01a\x02"[..]),
        )
        .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test_log::test(tokio::test)]
    async fn unit_stream_max_chunk() -> std::io::Result<()> {
        let s = crate::unpack(
            <BoxStream<'static, Vec<()>> as Decode>::Decoder::default(),
            &mut BytesMut::from(&b"\x01\xff\xff\xff\xff\x0f"[..]),
        )?;
        let lens: Vec<_> = s.map(|chunk| chunk.len()).collect().await;
        assert_eq!(lens, [0xffff_ffff]);
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
