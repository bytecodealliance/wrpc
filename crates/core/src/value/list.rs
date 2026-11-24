use core::mem;
use core::task::{Context, Poll};

use std::collections::BTreeMap;

use bytes::BytesMut;
use tracing::instrument;
use wasm_tokio::{DEFAULT_MAX_INITIAL_CAPACITY, Leb128DecoderU32};

use crate::value::encode_list;
use crate::{Decode, Encode, Receive, ReceiveError, Transmit};

/// Encoder for `list<T>`, `list<T, N>` and `map<K, V>`
pub struct ListEncoder<E> {
    pub(super) nested: BTreeMap<usize, E>,
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

impl<'a, 'b, T, E> tokio_util::codec::Encoder<&'a &'b T> for ListEncoder<E>
where
    T: ?Sized,
    &'b T: Encode<Encoder = Self>,
    <&'b T as Encode>::Encoder: tokio_util::codec::Encoder<&'b T, Error = std::io::Error>,
{
    type Error = std::io::Error;

    fn encode(&mut self, items: &'a &'b T, dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.encode(*items, dst)
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
    pub(super) cap: usize,
    pub(super) nested: BTreeMap<usize, T>,
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
    ) -> Poll<Result<(), ReceiveError>> {
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
    ) -> Poll<Result<(), ReceiveError>> {
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
