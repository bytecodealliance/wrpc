//! Packing and unpacking of wRPC values to/from singular, flat byte buffers

use std::collections::VecDeque;

use bytes::{BufMut as _, Bytes, BytesMut};
use futures::{Stream, StreamExt as _};
use tokio::io::{AsyncRead, AsyncReadExt as _};
use tokio::sync::mpsc;
use tokio_util::codec::{Decoder, Encoder};

use crate::{Decode, Deferred as _, DeferredFn, Encode, Index, Sink, encode_ready_bytes_header};

/// [Sink] used by [pack], which collects the packed asynchronous values
#[derive(Clone)]
pub struct Packer(mpsc::UnboundedSender<(usize, VecDeque<Bytes>)>);

impl Packer {
    fn push(&self, offset: usize, bufs: VecDeque<Bytes>) -> std::io::Result<()> {
        self.0
            .send((offset, bufs))
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::BrokenPipe))
    }

    fn push_bytes(&self, offset: usize, mut chunks: VecDeque<Bytes>) -> std::io::Result<()> {
        let mut header = BytesMut::default();
        encode_ready_bytes_header(chunks.iter().map(Bytes::len).sum(), &mut header)?;
        chunks.push_front(header.freeze());
        self.push(offset, chunks)
    }
}

impl Index for Packer {
    fn index(&self, _: &[usize]) -> std::io::Result<Self> {
        Ok(self.clone())
    }
}

impl Sink for Packer {
    async fn write_future<T>(
        self,
        offset: usize,
        item: impl Future<Output = T> + Send + 'static,
    ) -> std::io::Result<()>
    where
        T: Encode<Self>,
        std::io::Error: From<<T::Encoder as Encoder<T>>::Error>,
    {
        let mut buf = BytesMut::default();
        buf.put_u8(0x01);
        let deferred = T::encode(item.await, &mut T::Encoder::default(), &mut buf)?;
        fill(deferred, &mut buf).await?;
        self.push(offset, VecDeque::from([buf.freeze()]))
    }

    async fn write_stream<T>(
        self,
        offset: usize,
        items: impl Stream<Item = Vec<T>> + Send + Unpin + 'static,
    ) -> std::io::Result<()>
    where
        T: Encode<Self> + Send + 'static,
        std::io::Error: From<<T::Encoder as Encoder<T>>::Error>,
    {
        self.write_future(offset, items.concat()).await
    }

    async fn write_stream_bytes(
        self,
        offset: usize,
        items: impl Stream<Item = Bytes> + Send + Unpin + 'static,
    ) -> std::io::Result<()> {
        self.push_bytes(offset, items.collect().await)
    }

    async fn write_stream_read(
        self,
        offset: usize,
        mut ready: BytesMut,
        mut items: impl AsyncRead + Send + Unpin + 'static,
    ) -> std::io::Result<()> {
        while items.read_buf(&mut ready).await? > 0 {}
        self.push_bytes(offset, VecDeque::from([ready.freeze()]))
    }
}

async fn fill(deferred: Option<DeferredFn<Packer>>, dst: &mut BytesMut) -> std::io::Result<()> {
    let Some(f) = deferred else {
        return Ok(());
    };
    let (tx, mut rx) = mpsc::unbounded_channel();
    f(Packer(tx), Vec::default()).await?;
    let mut holes = Vec::default();
    while let Some(hole) = rx.recv().await {
        holes.push(hole);
    }
    holes.sort_unstable_by_key(|(offset, _)| *offset);
    let n = holes
        .iter()
        .flat_map(|(_, bufs)| bufs)
        .map(Bytes::len)
        .sum::<usize>();
    let mut out = BytesMut::with_capacity(dst.len() + n);
    let mut prev = 0;
    for (offset, bufs) in holes {
        if offset < prev || dst.get(offset) != Some(&0x00) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "asynchronous value was not encoded at the expected offset",
            ));
        }
        out.extend_from_slice(&dst[prev..offset]);
        for buf in bufs {
            out.extend_from_slice(&buf);
        }
        prev = offset + 1;
    }
    out.extend_from_slice(&dst[prev..]);
    *dst = out;
    Ok(())
}

/// Pack a [`Encode`] into a singular byte buffer `dst`.
///
/// All asynchronous values contained in `v` are awaited and encoded as ready values:
///
/// - `future<T>` is encoded as `ready` variant of `future<T>`
/// - `stream<T>` is encoded as `ready` variant of `stream<T>` containing all elements of the stream
///
/// Note, that this function will never complete if any of the asynchronous values contained in
/// `v` never resolve, use e.g. [`tokio::time::timeout`] to bound the time spent packing.
///
/// This function does not verify that all asynchronous values have been written. If a deferred
/// function returned by an encoder does not write every asynchronous value it is responsible for,
/// the corresponding pending placeholders are left in `dst` and [`unpack`] will fail.
///
/// If this function returns an error, contents of `dst` are undefined.
pub async fn pack<T: Encode<Packer>>(
    v: T,
    dst: &mut BytesMut,
) -> Result<(), <T::Encoder as Encoder<T>>::Error> {
    let deferred = v.encode(&mut T::Encoder::default(), dst)?;
    fill(deferred, dst).await?;
    Ok(())
}

/// Synchronously pack a [`Encode`] into a singular byte buffer `dst`.
///
/// All asynchronous values contained in `v` must be ready, this function returns an error of
/// kind [`std::io::ErrorKind::WouldBlock`] if any of them is pending.
///
/// Futures are polled exactly once and streams are polled until they are pending or finished,
/// so values backed by Tokio I/O types (e.g. `tokio::fs::File`) are typically pending and require
/// this function to be called within the context of a Tokio runtime.
///
/// If this function returns an error, contents of `dst` are undefined.
pub fn pack_ready<T: Encode<Packer>>(
    v: T,
    dst: &mut BytesMut,
) -> Result<(), <T::Encoder as Encoder<T>>::Error> {
    let deferred = v.encode(&mut T::Encoder::default(), dst)?;
    if deferred.is_some() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "asynchronous value is pending",
        )
        .into());
    }
    Ok(())
}

/// Unpack a [`Decode`] from a byte buffer `buf` produced by [`pack`] or [`pack_ready`].
///
/// This function returns an error if `buf` contains pending asynchronous values or if `buf` is
/// not fully consumed.
///
/// If this function returns an error, contents of `buf` are undefined.
pub fn unpack<T: Decode>(buf: &mut BytesMut) -> Result<T, <T::Decoder as Decoder>::Error> {
    let mut dec = T::Decoder::default();
    let Some(v) = dec.decode(buf)? else {
        return Err(
            std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "buffer is incomplete").into(),
        );
    };
    if dec.take_deferred().is_some() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "buffer contains pending asynchronous values, which cannot be unpacked",
        )
        .into());
    }
    if !buf.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "buffer contains trailing bytes",
        )
        .into());
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use core::future::{pending, ready};
    use core::pin::Pin;

    use futures::FutureExt as _;
    use futures::future::{self, BoxFuture};
    use futures::stream::{self, BoxStream};
    use tokio::task::yield_now;
    use tokio_util::io::StreamReader;

    use super::*;

    #[test_log::test(tokio::test)]
    async fn roundtrip() -> anyhow::Result<()> {
        type Value = (
            u8,
            BoxFuture<'static, String>,
            BoxStream<'static, Vec<BoxFuture<'static, u8>>>,
            BoxStream<'static, Bytes>,
            Pin<Box<dyn AsyncRead + Send>>,
            Vec<BoxFuture<'static, Option<BoxStream<'static, Vec<String>>>>>,
            BoxStream<'static, Vec<u8>>,
        );

        fn later<T: Send + 'static>(v: T) -> BoxFuture<'static, T> {
            async {
                yield_now().await;
                v
            }
            .boxed()
        }

        fn chunks<T: Send + 'static>(ready: T, pending: T) -> BoxStream<'static, T> {
            stream::iter([ready])
                .chain(stream::once(later(pending)))
                .boxed()
        }

        let v: Value = (
            0x42,
            later("foo".into()),
            stream::iter([vec![ready(1).boxed(), later(2)], vec![], vec![later(3)]]).boxed(),
            chunks(Bytes::from("foo"), Bytes::from("bar")),
            Box::pin(StreamReader::new(
                chunks(Bytes::from("foo"), Bytes::from("bar")).map(Ok::<_, std::io::Error>),
            )),
            vec![
                later(Some(chunks(vec!["foo".into()], vec!["bar".into()]))),
                ready(None).boxed(),
            ],
            stream::empty().boxed(),
        );
        let mut buf = BytesMut::default();
        pack(v, &mut buf).await?;
        let (a, b, c, d, mut e, f, g) = unpack::<Value>(&mut buf)?;
        assert_eq!(a, 0x42);
        assert_eq!(b.await, "foo");
        assert_eq!(future::join_all(c.concat().await).await, [1, 2, 3]);
        let d: Vec<_> = d.collect().await;
        assert_eq!(d.concat(), b"foobar");
        let mut s = String::default();
        e.read_to_string(&mut s).await?;
        assert_eq!(s, "foobar");
        let [f0, f1] = <[_; 2]>::try_from(future::join_all(f).await)
            .ok()
            .expect("invalid list length");
        assert_eq!(f0.expect("missing stream").concat().await, ["foo", "bar"]);
        assert!(f1.is_none());
        assert_eq!(g.count().await, 0);

        let s = stream::iter((0..=255_u8).map(|i| vec![ready(i).boxed()])).boxed();
        pack_ready(s, &mut buf)?;
        let s = unpack::<BoxStream<'static, Vec<BoxFuture<'static, u8>>>>(&mut buf)?;
        assert_eq!(
            future::join_all(s.concat().await).await,
            (0..=255).collect::<Vec<u8>>()
        );
        Ok(())
    }

    #[test]
    fn rejects_invalid() {
        let fut = ready(pending::<u8>().boxed()).boxed();
        let err = pack_ready(fut, &mut BytesMut::default()).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock);

        let err = unpack::<BoxFuture<'static, u8>>(&mut BytesMut::from("\0"))
            .err()
            .unwrap();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

        let err = unpack::<u8>(&mut BytesMut::from("\x42\0")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }
}
