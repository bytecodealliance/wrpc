use core::future::Future;
use core::iter::zip;
use core::mem;
use core::pin::Pin;

use bytes::{Buf as _, BufMut as _, Bytes, BytesMut};
use futures::stream::{self, FuturesUnordered};
use futures::{Stream, StreamExt as _, TryStreamExt as _};
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt as _};
use tokio::sync::mpsc;
use tokio::try_join;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::codec::FramedRead;
use tracing::instrument;
use wasm_tokio::cm::{
    BoolCodec, F32Codec, F64Codec, S16Codec, S32Codec, S64Codec, S8Codec, TupleDecoder, U16Codec,
    U32Codec, U64Codec, U8Codec,
};
use wasm_tokio::{
    CoreNameDecoder, CoreNameEncoder, CoreVecDecoderBytes, CoreVecEncoderBytes, Leb128DecoderU32,
    Leb128Encoder, Utf8Codec,
};

pub struct ValueEncoder<W>(
    Option<
        Box<
            dyn FnOnce(W) -> Pin<Box<dyn Future<Output = std::io::Result<()>> + Send + Sync>>
                + Send
                + Sync,
        >,
    >,
);

impl<W> Default for ValueEncoder<W> {
    fn default() -> Self {
        Self(None)
    }
}

impl<W> ValueEncoder<W> {
    pub fn set_deferred(
        &mut self,
        f: Box<
            dyn FnOnce(W) -> Pin<Box<dyn Future<Output = std::io::Result<()>> + Send + Sync>>
                + Send
                + Sync,
        >,
    ) -> Option<
        Box<
            dyn FnOnce(W) -> Pin<Box<dyn Future<Output = std::io::Result<()>> + Send + Sync>>
                + Send
                + Sync,
        >,
    > {
        self.0.replace(f)
    }

    pub fn take_deferred(
        &mut self,
    ) -> Option<
        Box<
            dyn FnOnce(W) -> Pin<Box<dyn Future<Output = std::io::Result<()>> + Send + Sync>>
                + Send
                + Sync,
        >,
    > {
        self.0.take()
    }

    pub async fn write_deferred(self, w: W) -> std::io::Result<()> {
        let Some(f) = self.0 else { return Ok(()) };
        f(w).await
    }
}

pub trait Decode: Send + Sync {
    type State<R>: Default + Send + Sync;
}

pub struct ValueDecoder<R, T: Decode> {
    state: T::State<R>,
    index: Vec<usize>,
    deferred: Option<
        Box<
            dyn FnOnce(R) -> Pin<Box<dyn Future<Output = std::io::Result<()>> + Send + Sync>>
                + Send
                + Sync,
        >,
    >,
}

impl<R, T: Decode> Default for ValueDecoder<R, T> {
    fn default() -> Self {
        Self {
            state: T::State::<R>::default(),
            index: Vec::default(),
            deferred: None,
        }
    }
}

impl<R, T: Decode> ValueDecoder<R, T> {
    #[must_use]
    pub fn new(index: Vec<usize>) -> Self {
        Self {
            state: T::State::default(),
            index,
            deferred: None,
        }
    }

    pub fn set_deferred(
        &mut self,
        f: Box<
            dyn FnOnce(R) -> Pin<Box<dyn Future<Output = std::io::Result<()>> + Send + Sync>>
                + Send
                + Sync,
        >,
    ) -> Option<
        Box<
            dyn FnOnce(R) -> Pin<Box<dyn Future<Output = std::io::Result<()>> + Send + Sync>>
                + Send
                + Sync,
        >,
    > {
        self.deferred.replace(f)
    }

    pub fn take_deferred(
        &mut self,
    ) -> Option<
        Box<
            dyn FnOnce(R) -> Pin<Box<dyn Future<Output = std::io::Result<()>> + Send + Sync>>
                + Send
                + Sync,
        >,
    > {
        self.deferred.take()
    }

    pub async fn read_deferred(self, r: R) -> std::io::Result<()> {
        let Some(f) = self.deferred else {
            return Ok(());
        };
        f(r).await
    }
}

macro_rules! impl_copy_codec {
    ($t:ty, $c:expr) => {
        impl<W> tokio_util::codec::Encoder<$t> for ValueEncoder<W> {
            type Error = std::io::Error;

            #[instrument(level = "trace", skip(self), ret)]
            fn encode(&mut self, item: $t, dst: &mut BytesMut) -> std::io::Result<()> {
                $c.encode(item, dst)
            }
        }

        impl<W> tokio_util::codec::Encoder<&$t> for ValueEncoder<W> {
            type Error = std::io::Error;

            #[instrument(level = "trace", skip(self), ret)]
            fn encode(&mut self, item: &$t, dst: &mut BytesMut) -> std::io::Result<()> {
                $c.encode(*item, dst)
            }
        }

        impl<R> tokio_util::codec::Decoder for ValueDecoder<R, $t> {
            type Item = $t;
            type Error = std::io::Error;

            #[instrument(level = "trace", skip(self), ret)]
            fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
                $c.decode(src)
            }
        }

        impl Decode for $t {
            type State<R> = ();
        }
    };
}

impl_copy_codec!(bool, BoolCodec);
impl_copy_codec!(i8, S8Codec);
impl_copy_codec!(u8, U8Codec);
impl_copy_codec!(i16, S16Codec);
impl_copy_codec!(u16, U16Codec);
impl_copy_codec!(i32, S32Codec);
impl_copy_codec!(u32, U32Codec);
impl_copy_codec!(i64, S64Codec);
impl_copy_codec!(u64, U64Codec);
impl_copy_codec!(f32, F32Codec);
impl_copy_codec!(f64, F64Codec);
impl_copy_codec!(char, Utf8Codec);

macro_rules! impl_string_encode {
    ($t:ty) => {
        impl<W> tokio_util::codec::Encoder<$t> for ValueEncoder<W> {
            type Error = std::io::Error;

            #[instrument(level = "trace", skip(self), ret)]
            fn encode(&mut self, item: $t, dst: &mut BytesMut) -> std::io::Result<()> {
                CoreNameEncoder.encode(item, dst)
            }
        }

        impl<W> tokio_util::codec::Encoder<&$t> for ValueEncoder<W> {
            type Error = std::io::Error;

            #[instrument(level = "trace", skip(self), ret)]
            fn encode(&mut self, item: &$t, dst: &mut BytesMut) -> std::io::Result<()> {
                CoreNameEncoder.encode(item, dst)
            }
        }
    };
}

impl_string_encode!(&str);
impl_string_encode!(String);

impl<R> tokio_util::codec::Decoder for ValueDecoder<R, String> {
    type Item = String;
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self), ret)]
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        self.state.decode(src)
    }
}

impl Decode for String {
    type State<R> = CoreNameDecoder;
}

impl<W> tokio_util::codec::Encoder<Bytes> for ValueEncoder<W> {
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self), ret)]
    fn encode(&mut self, item: Bytes, dst: &mut BytesMut) -> std::io::Result<()> {
        CoreVecEncoderBytes.encode(item, dst)
    }
}

impl<W> tokio_util::codec::Encoder<&Bytes> for ValueEncoder<W> {
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self), ret)]
    fn encode(&mut self, item: &Bytes, dst: &mut BytesMut) -> std::io::Result<()> {
        CoreVecEncoderBytes.encode(item.as_ref(), dst)
    }
}

impl<R> tokio_util::codec::Decoder for ValueDecoder<R, Bytes> {
    type Item = Bytes;
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self), ret)]
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        self.state.decode(src)
    }
}

impl Decode for Bytes {
    type State<R> = CoreVecDecoderBytes;
}

async fn handle_deferred<T, I>(w: T, deferred: I) -> std::io::Result<()>
where
    T: crate::Index<T> + 'static,
    I: IntoIterator,
    I::IntoIter: ExactSizeIterator<
        Item = Option<
            Box<
                dyn FnOnce(T) -> Pin<Box<dyn Future<Output = std::io::Result<()>> + Send + Sync>>
                    + Send
                    + Sync,
            >,
        >,
    >,
{
    let futs: FuturesUnordered<_> = zip(0.., deferred)
        .filter_map(|(i, f)| f.map(|f| (w.index(&[i]), f)))
        .map(|(w, f)| async move {
            let w = w.map_err(std::io::Error::other)?;
            f(w).await
        })
        .collect();
    futs.try_collect().await?;
    Ok(())
}

macro_rules! impl_tuple_codec {
    ($($vn:ident),+; $($vt:ident),+; $($cn:ident),+) => {
        impl<E, W, $($vt),+> tokio_util::codec::Encoder<($($vt),+,)> for ValueEncoder<W>
        where
            E: From<std::io::Error>,
            W: AsyncWrite + crate::Index<W> + Send + Sync + 'static,
            std::io::Error: From<E>,
            $(Self: tokio_util::codec::Encoder<$vt, Error = E>),+
        {
            type Error = std::io::Error;

            #[instrument(level = "trace", skip_all, ret, fields(ty = "tuple", dst))]
            fn encode(&mut self, ($($vn),+,): ($($vt),+,), dst: &mut BytesMut) -> std::io::Result<()> {
                $(
                    let mut $cn = Self::default();
                    $cn.encode($vn, dst)?;
                )+
                let deferred = [ $($cn.take_deferred()),+ ];
                if deferred.iter().any(Option::is_some) {
                    self.0 = Some(Box::new(|w| Box::pin(handle_deferred(w, deferred))));
                }
                Ok(())
            }
        }

        impl<E, R, $($vt),+> tokio_util::codec::Decoder for ValueDecoder<R, ($($vt),+,)>
        where
            E: From<std::io::Error>,
            R: AsyncRead + crate::Index<R> + Send + Sync + Unpin + 'static,
            std::io::Error: From<E>,
            $(ValueDecoder<R, $vt>: tokio_util::codec::Decoder<Item = $vt, Error = E>),+,
            $($vt: Decode),+
        {
            type Error = std::io::Error;
            type Item = ($($vt),+,);

            #[instrument(level = "trace", skip(self))]
            fn decode(
                &mut self,
                src: &mut BytesMut,
            ) -> Result<Option<Self::Item>, Self::Error> {
                let Some(ret) = self.state.decode(src)? else {
                    return Ok(None)
                };
                let ($(mut $cn),+,) = mem::take(&mut self.state).into_inner();
                let deferred = [ $($cn.take_deferred()),+ ];
                if deferred.iter().any(Option::is_some) {
                    self.deferred = Some(Box::new(|r| Box::pin(handle_deferred(r, deferred))));
                }
                Ok(Some(ret))
            }
        }

        impl<$($vt),+> Decode for ($($vt),+,) where
            $($vt: Decode),+
        {
            type State<R> = TupleDecoder::<($(ValueDecoder<R, $vt>),+,), ($(Option<$vt>),+,)>;
        }
    };
}

impl_tuple_codec!(
    v0;
    V0;
    c0
);

impl_tuple_codec!(
    v0, v1;
    V0, V1;
    c0, c1
);

impl_tuple_codec!(
    v0, v1, v2;
    V0, V1, V2;
    c0, c1, c2
);

impl_tuple_codec!(
    v0, v1, v2, v3;
    V0, V1, V2, V3;
    c0, c1, c2, c3
);

impl_tuple_codec!(
    v0, v1, v2, v3, v4;
    V0, V1, V2, V3, V4;
    c0, c1, c2, c3, c4
);

impl_tuple_codec!(
    v0, v1, v2, v3, v4, v5;
    V0, V1, V2, V3, V4, V5;
    c0, c1, c2, c3, c4, c5
);

impl_tuple_codec!(
    v0, v1, v2, v3, v4, v5, v6;
    V0, V1, V2, V3, V4, V5, V6;
    c0, c1, c2, c3, c4, c5, c6
);

impl_tuple_codec!(
    v0, v1, v2, v3, v4, v5, v6, v7;
    V0, V1, V2, V3, V4, V5, V6, V7;
    c0, c1, c2, c3, c4, c5, c6, c7
);

impl_tuple_codec!(
    v0, v1, v2, v3, v4, v5, v6, v7, v8;
    V0, V1, V2, V3, V4, V5, V6, V7, V8;
    c0, c1, c2, c3, c4, c5, c6, c7, c8
);

impl_tuple_codec!(
    v0, v1, v2, v3, v4, v5, v6, v7, v8, v9;
    V0, V1, V2, V3, V4, V5, V6, V7, V8, V9;
    c0, c1, c2, c3, c4, c5, c6, c7, c8, c9
);

impl_tuple_codec!(
    v0, v1, v2, v3, v4, v5, v6, v7, v8, v9, v10;
    V0, V1, V2, V3, V4, V5, V6, V7, V8, V9, V10;
    c0, c1, c2, c3, c4, c5, c6, c7, c8, c9, c10
);

impl_tuple_codec!(
    v0, v1, v2, v3, v4, v5, v6, v7, v8, v9, v10, v11;
    V0, V1, V2, V3, V4, V5, V6, V7, V8, V9, V10, V11;
    c0, c1, c2, c3, c4, c5, c6, c7, c8, c9, c10, c11
);

impl<T, W> tokio_util::codec::Encoder<Vec<T>> for ValueEncoder<W>
where
    Self: tokio_util::codec::Encoder<T>,
    W: AsyncWrite + crate::Index<W> + Send + Sync + 'static,
    <Self as tokio_util::codec::Encoder<T>>::Error: Into<std::io::Error>,
{
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self, item), fields(ty = "list"))]
    fn encode(&mut self, item: Vec<T>, dst: &mut BytesMut) -> std::io::Result<()> {
        let items = item.len();
        let n = u32::try_from(items)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
        dst.reserve(5 + items);
        Leb128Encoder.encode(n, dst)?;
        let mut deferred = Vec::with_capacity(items);
        for item in item {
            let mut enc = ValueEncoder::default();
            enc.encode(item, dst).map_err(Into::into)?;
            deferred.push(enc.0);
        }
        if deferred.iter().any(Option::is_some) {
            self.0 = Some(Box::new(|w| Box::pin(handle_deferred(w, deferred))));
        }
        Ok(())
    }
}

impl<T: Copy, W> tokio_util::codec::Encoder<&[T]> for ValueEncoder<W>
where
    Self: tokio_util::codec::Encoder<T, Error = std::io::Error>,
    W: AsyncWrite + crate::Index<W> + Send + Sync + 'static,
{
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self, item), fields(ty = "list"))]
    fn encode(&mut self, item: &[T], dst: &mut BytesMut) -> std::io::Result<()> {
        let items = item.len();
        let n = u32::try_from(items)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
        dst.reserve(5 + items);
        Leb128Encoder.encode(n, dst)?;
        let mut deferred = Vec::with_capacity(items);
        for item in item {
            let mut enc = ValueEncoder::default();
            enc.encode(*item, dst)?;
            deferred.push(enc.0);
        }
        if deferred.iter().any(Option::is_some) {
            self.0 = Some(Box::new(|w| Box::pin(handle_deferred(w, deferred))));
        }
        Ok(())
    }
}

impl<T, W> tokio_util::codec::Encoder<Pin<Box<dyn Future<Output = T> + Send + Sync>>>
    for ValueEncoder<W>
where
    Self: tokio_util::codec::Encoder<T>,
    W: AsyncWrite + crate::Index<W> + Send + Sync + Unpin + 'static,
    T: 'static,
    std::io::Error: From<<Self as tokio_util::codec::Encoder<T>>::Error>,
{
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self, item), fields(ty = "future"))]
    fn encode(
        &mut self,
        item: Pin<Box<dyn Future<Output = T> + Send + Sync>>,
        dst: &mut BytesMut,
    ) -> std::io::Result<()> {
        dst.reserve(1);
        dst.put_u8(0x00);
        self.0 = Some(Box::new(|w| {
            Box::pin(async move {
                let mut w = w.index(&[0]).map_err(std::io::Error::other)?;
                let item = item.await;
                let mut enc = ValueEncoder::default();
                let mut buf = BytesMut::default();
                enc.encode(item, &mut buf)?;
                w.write_all(&buf).await?;
                enc.write_deferred(w).await?;
                Ok(())
            })
        }));
        Ok(())
    }
}

impl<T, W> tokio_util::codec::Encoder<Pin<Box<dyn Stream<Item = T> + Send + Sync>>>
    for ValueEncoder<W>
where
    Self: tokio_util::codec::Encoder<T>,
    W: AsyncWrite + crate::Index<W> + Send + Sync + Unpin + 'static,
    T: Send + Sync + 'static,
    std::io::Error: From<<Self as tokio_util::codec::Encoder<T>>::Error>,
{
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self, item), fields(ty = "stream"))]
    fn encode(
        &mut self,
        item: Pin<Box<dyn Stream<Item = T> + Send + Sync>>,
        dst: &mut BytesMut,
    ) -> std::io::Result<()> {
        dst.reserve(1);
        dst.put_u8(0x00);
        self.0 = Some(Box::new(|w| {
            let w = Arc::new(w);
            Box::pin(
                item.enumerate()
                    .map(Ok)
                    .try_for_each_concurrent(None, move |(i, item)| {
                        let w = Arc::clone(&w);
                        async move {
                            let mut w = w.index(&[i]).map_err(std::io::Error::other)?;
                            let mut enc = ValueEncoder::<W>::default();
                            let mut buf = BytesMut::default();
                            enc.encode(item, &mut buf)?;
                            w.write_all(&buf).await?;
                            enc.write_deferred(w).await?;
                            Ok(())
                        }
                    }),
            )
        }));
        Ok(())
    }
}

impl<T: Decode> Decode for Pin<Box<dyn Stream<Item = T> + Send + Sync>> {
    type State<R> = Option<StreamChunkDecodeState<T>>;
}

impl<T: Decode> Decode for StreamChunk<T> {
    type State<R> = StreamChunkDecodeState<T>;
}

struct StreamChunk<T>(Vec<T>);

impl<T> Default for StreamChunk<T> {
    fn default() -> Self {
        Self(Vec::default())
    }
}

pub struct StreamChunkDecodeState<T> {
    offset: Option<usize>,
    ret: Vec<T>,
    cap: usize,
}

impl<T> Default for StreamChunkDecodeState<T> {
    fn default() -> Self {
        Self {
            offset: None,
            ret: Vec::default(),
            cap: 0,
        }
    }
}

impl<R, T> tokio_util::codec::Decoder for ValueDecoder<R, StreamChunk<T>>
where
    ValueDecoder<R, T>: tokio_util::codec::Decoder<Item = T>,
    R: AsyncRead + crate::Index<R> + Send + Sync + Unpin + 'static,
    T: Decode,
    std::io::Error: From<<ValueDecoder<R, T> as tokio_util::codec::Decoder>::Error>,
{
    type Item = StreamChunk<T>;
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self), fields(ty = "stream"))]
    fn decode(&mut self, src: &mut BytesMut) -> std::io::Result<Option<Self::Item>> {
        if self.state.cap == 0 {
            let Some(len) = Leb128DecoderU32.decode(src)? else {
                return Ok(None);
            };
            if len == 0 {
                mem::take(&mut self.state);
                return Ok(Some(StreamChunk::default()));
            }
            let len = len
                .try_into()
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
            self.state.ret = Vec::with_capacity(len);
            self.state.cap = len;
        }
        while self.state.cap > 0 {
            let mut index = mem::take(&mut self.index);
            let offset = self.state.offset.ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "stream element offset overflows usize",
                )
            })?;
            let i = self.state.ret.len().checked_add(offset).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "stream element index overflows usize",
                )
            })?;
            index.push(i);
            let mut dec = ValueDecoder::<R, T>::new(index);
            let res = dec.decode(src);
            let mut index = dec.index;
            let Some(v) = res? else {
                index.pop();
                self.index = index;
                return Ok(None);
            };
            if let Some(deferred) = dec.deferred {
                let index = Arc::from(index.as_ref());
                if let Some(f) = self.deferred.take() {
                    self.deferred = Some(Box::new(|r| {
                        Box::pin(async move {
                            let indexed = r.index(&index).map_err(std::io::Error::other)?;
                            try_join!(f(r), deferred(indexed))?;
                            Ok(())
                        })
                    }));
                } else {
                    self.deferred = Some(Box::new(|r| {
                        Box::pin(async move {
                            let indexed = r.index(&index).map_err(std::io::Error::other)?;
                            deferred(indexed).await
                        })
                    }));
                }
            }
            index.pop();
            self.index = index;
            self.state.ret.push(v);
            self.state.cap -= 1;
            self.state.offset = offset.checked_add(1);
        }
        let StreamChunkDecodeState { ret, .. } = mem::take(&mut self.state);
        Ok(Some(StreamChunk(ret)))
    }
}

impl<R, T> tokio_util::codec::Decoder
    for ValueDecoder<R, Pin<Box<dyn Stream<Item = T> + Send + Sync>>>
where
    ValueDecoder<R, T>: tokio_util::codec::Decoder<Item = T>,
    R: AsyncRead + crate::Index<R> + Send + Sync + Unpin + 'static,
    T: Decode + Send + Sync + 'static,
    std::io::Error: From<<ValueDecoder<R, T> as tokio_util::codec::Decoder>::Error>,
{
    type Item = Pin<Box<dyn Stream<Item = T> + Send + Sync>>;
    type Error = std::io::Error;

    #[instrument(level = "trace", skip(self), fields(ty = "stream"))]
    fn decode(&mut self, src: &mut BytesMut) -> std::io::Result<Option<Self::Item>> {
        let state = if let Some(state) = self.state.take() {
            state
        } else {
            if src.is_empty() {
                src.reserve(1);
                return Ok(None);
            }
            match src.get_u8() {
                0 => {
                    let index = self.index.clone();
                    let (tx, rx) = mpsc::channel(128);
                    self.deferred = Some(Box::new(|r| {
                        Box::pin(async move {
                            let indexed = r.index(&index).map_err(std::io::Error::other)?;
                            let mut r = FramedRead::new(
                                indexed,
                                ValueDecoder::<R, StreamChunk<T>>::new(index),
                            );
                            while let Some(StreamChunk(chunk)) = r.try_next().await? {
                                if chunk.is_empty() {
                                    return Ok(());
                                }
                                for v in chunk {
                                    tx.send(v).await.map_err(|_| {
                                        std::io::Error::new(
                                            std::io::ErrorKind::BrokenPipe,
                                            "receiver closed",
                                        )
                                    })?;
                                }
                            }
                            Ok(())
                        })
                    }));
                    return Ok(Some(Box::pin(ReceiverStream::new(rx))));
                }
                1 => StreamChunkDecodeState::default(),
                n => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("invalid stream status byte `{n}`"),
                    ))
                }
            }
        };
        let index = mem::take(&mut self.index);
        let mut dec = ValueDecoder::<R, StreamChunk<T>> {
            state,
            index,
            deferred: None,
        };
        let res = dec.decode(src)?;
        self.index = dec.index;
        if let Some(StreamChunk(chunk)) = res {
            Ok(Some(Box::pin(stream::iter(chunk))))
        } else {
            self.state = Some(dec.state);
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio_util::codec::Encoder as _;

    use super::*;

    #[test_log::test(tokio::test)]
    async fn value_encoder() -> std::io::Result<()> {
        let mut buf = BytesMut::new();
        ValueEncoder::<()>::default().encode(0x42u8, &mut buf)?;
        assert_eq!(buf.as_ref(), b"\x42");
        Ok(())
    }
}
