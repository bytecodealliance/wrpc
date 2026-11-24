use core::hash::{BuildHasher, Hash};
use core::marker::PhantomData;
use core::task::{Context, Poll};

use std::collections::{BTreeMap, HashMap};

use bytes::BytesMut;
use tokio_util::codec::Encoder as _;
use tracing::instrument;
use wasm_tokio::Leb128Encoder;
use wasm_tokio::cm::TupleEncoder;

use crate::value::encode_items;
use crate::{Decode, Encode, ListDecoder, ListEncoder, Receive, ReceiveError, Transmit};

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
    ) -> Poll<Result<(), ReceiveError>> {
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
