use core::fmt::Debug;
use core::hash::Hash;
use core::mem;
use core::ops::{Deref, DerefMut};
use core::str;
use core::task::{Context, Poll, ready};

use std::collections::BTreeMap;

use bytes::{Bytes, BytesMut};
use tokio_util::codec::{Decoder as _, Encoder as _};
use tracing::instrument;
use wasm_tokio::cm::{
    BoolCodec, F32Codec, F64Codec, OptionDecoder, OptionEncoder, PrimValEncoder, ResultDecoder,
    ResultEncoder, S8Codec, S16Codec, S32Codec, S64Codec, U8Codec, U16Codec, U32Codec, U64Codec,
};
use wasm_tokio::{
    CoreNameDecoder, CoreNameEncoder, CoreVecDecoder, CoreVecDecoderBytes, CoreVecEncoderBytes,
    Leb128DecoderI8, Leb128DecoderI16, Leb128DecoderI32, Leb128DecoderI64, Leb128DecoderI128,
    Leb128DecoderU8, Leb128DecoderU16, Leb128DecoderU32, Leb128DecoderU64, Leb128DecoderU128,
    Leb128Encoder, Utf8Codec,
};

use crate::{ReceiveError, error_unknown_path};

mod future;
mod list;
mod map;
mod resource;
mod stream;
mod tuple;

pub use future::*;
pub use list::*;
pub use map::*;
pub use resource::*;
pub use stream::*;
pub use tuple::*;

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
    ) -> Poll<Result<(), ReceiveError>>;

    /// Returns whether nothing in the last decoded value is pending
    fn is_complete(&self) -> bool;
}

impl<T: Receive + ?Sized> Receive for Box<T> {
    fn poll_receive(
        &mut self,
        cx: &mut Context<'_>,
        path: &[usize],
        src: &mut BytesMut,
    ) -> Poll<Result<(), ReceiveError>> {
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
    ) -> Poll<Result<(), ReceiveError>> {
        let Some((i, rest)) = path.split_first() else {
            return Poll::Ready(Err(error_unknown_path(path).into()));
        };
        let Some(r) = self.get_mut(i) else {
            return Poll::Ready(Err(error_unknown_path(path).into()));
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
        ) -> Poll<Result<(), ReceiveError>> {
            Poll::Ready(Err(error_unknown_path(path).into()))
        }

        fn is_complete(&self) -> bool {
            true
        }
    };
}

use sync_receive;

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
    ) -> Poll<Result<(), ReceiveError>> {
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
    ) -> Poll<Result<(), ReceiveError>> {
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

#[cfg(test)]
mod tests {
    use core::future::pending;
    use core::time::Duration;

    use std::collections::HashMap;
    use std::rc::Rc;

    use anyhow::bail;
    use futures::future::{BoxFuture, LocalBoxFuture, join_all};
    use futures::stream::{self, BoxStream, LocalBoxStream};
    use futures::{FutureExt as _, StreamExt as _};
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
            BTreeMap<String, BoxFuture<'static, u8>>,
        );

        fn delay<T: Send + 'static>(v: T) -> BoxFuture<'static, T> {
            Box::pin(async move {
                yield_now().await;
                v
            })
        }

        let v: V = (
            [[1, 300], [2, 3]],
            vec![delay(1)],
            delay(vec![delay(2)]),
            delay(()),
            Box::pin(stream::iter(["foo", "", "bar"].map(Bytes::from)).then(delay)),
            BTreeMap::from([("a".into(), delay(4)), ("b".into(), delay(5))]),
        );
        let (tx, rx) = tokio::io::duplex(1024);
        let tx = transmit(
            <V as Encode>::Encoder::default(),
            FramedWrite::new(tx, FrameEncoder),
            v,
        )?;
        let tx = spawn(tx);
        let ((a, b, c, d, e, f), rx) = receive(
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
        let (keys, values): (Vec<_>, Vec<_>) = f.into_iter().unzip();
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
        );
        let (tx, rx) = tokio::io::duplex(1024);
        let tx = transmit(
            <V as Encode>::Encoder::default(),
            FramedWrite::new(tx, FrameEncoder),
            v,
        )?;
        let rx = async {
            let ((a, b, c), rx) = receive(
                <V as Decode>::Decoder::default(),
                FramedRead::new(rx, FrameDecoder::default()),
            )
            .await?;
            let (v, ()) = try_join!(
                async {
                    let (a, b, c) = join!(a, b.concat(), c.collect::<Vec<Bytes>>());
                    Ok((a, join_all(b).await, c.concat()))
                },
                rx
            )?;
            std::io::Result::Ok(v)
        };
        let ((), (a, b, c)) = try_join!(tx, rx)?;
        assert_eq!(a, 1);
        assert_eq!(b, [2, 3]);
        assert_eq!(c, b"foo");
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
        ) -> std::io::Result<(T, impl Future<Output = Result<(), ReceiveError>> + use<T>)>
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
        assert!(
            matches!(&err, ReceiveError::Io(err) if err.kind() == std::io::ErrorKind::InvalidData),
            "{err:?}"
        );

        let (_fut, rx) = receive_value::<BoxFuture<'static, Bytes>>(b"\x03foo\x2a").await?;
        let err = rx.await.unwrap_err();
        assert!(
            matches!(&err, ReceiveError::Io(err) if err.kind() == std::io::ErrorKind::InvalidData),
            "{err:?}"
        );

        let (fut, rx) = receive_value::<BoxFuture<'static, Bytes>>(b"\x03foo").await?;
        drop(fut);
        let err = rx.await.unwrap_err();
        assert!(matches!(err, ReceiveError::Unread), "{err:?}");
        Ok(())
    }

    #[test]
    fn map_encodes_as_list() -> std::io::Result<()> {
        let v: HashMap<String, u32> = HashMap::from([("a".into(), 1)]);
        let mut buf = BytesMut::default();
        crate::pack(v, &mut buf)?;
        assert_eq!(buf, b"\x01\x01a\x01"[..]);
        let v: BTreeMap<String, u32> = crate::unpack(&mut buf)?;
        assert_eq!(v, BTreeMap::from([("a".into(), 1)]));
        Ok(())
    }

    #[test]
    fn map_rejects_duplicate_keys() {
        let err = crate::unpack::<HashMap<String, u32>>(&mut BytesMut::from(
            &b"\x02\x01a\x01\x01a\x02"[..],
        ))
        .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test_log::test(tokio::test)]
    async fn unit_stream_max_chunk() -> std::io::Result<()> {
        let s: BoxStream<'static, Vec<()>> =
            crate::unpack(&mut BytesMut::from(&b"\x01\xff\xff\xff\xff\x0f"[..]))?;
        let lens: Vec<_> = s.map(|chunk| chunk.len()).collect().await;
        assert_eq!(lens, [0xffff_ffff]);
        Ok(())
    }

    #[test]
    fn list_length_exceeds_data() {
        const LEN: &[u8] = b"\xff\xff\xff\xff\x0f";

        let err = crate::unpack::<Vec<String>>(&mut BytesMut::from(LEN)).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
        let err = crate::unpack::<Bytes>(&mut BytesMut::from(LEN)).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
    }
}
