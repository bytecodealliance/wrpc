//! Packing and unpacking of wRPC values to/from singular, flat byte buffers

use core::future::ready;

use bytes::{BufMut as _, Bytes, BytesMut};
use futures::{Stream, StreamExt as _};
use tokio::io::{AsyncRead, AsyncReadExt as _};
use tokio::sync::mpsc;
use tokio_util::codec::{Decoder, Encoder};

use crate::{Decode, Deferred as _, DeferredFn, Encode, Index, Sink};

/// [Sink] used by [pack], which collects the packed asynchronous values
#[derive(Clone)]
pub struct Packer(mpsc::UnboundedSender<(usize, BytesMut)>);

impl Packer {
    fn push(&self, offset: usize, buf: BytesMut) -> std::io::Result<()> {
        self.0
            .send((offset, buf))
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::BrokenPipe))
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
        self.push(offset, buf)
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
        self.write_future(offset, async { items.collect::<Vec<_>>().await.concat() })
            .await
    }

    async fn write_stream_read(
        self,
        offset: usize,
        mut items: impl AsyncRead + Send + Unpin + 'static,
    ) -> std::io::Result<()> {
        let mut payload = Vec::default();
        items.read_to_end(&mut payload).await?;
        self.write_future(offset, ready(payload)).await
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
    let n = holes.iter().map(|(_, buf)| buf.len()).sum::<usize>();
    let mut out = BytesMut::with_capacity(dst.len() + n);
    let mut prev = 0;
    for (offset, buf) in holes {
        if offset < prev || dst.get(offset) != Some(&0x00) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "asynchronous value was not encoded at the expected offset",
            ));
        }
        out.extend_from_slice(&dst[prev..offset]);
        out.extend_from_slice(&buf);
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
/// - `future<T>` is encoded as `option::some` variant of `option<T>`
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

/// Unpack a [`Decode`] from a byte buffer `buf` produced by [`pack`].
///
/// This function returns an error if `buf` contains pending asynchronous values or if `buf` is
/// not fully consumed.
///
/// If this function returns an error, contents of `buf` are undefined.
pub fn unpack<T: Decode>(buf: &mut BytesMut) -> Result<T, <T::Decoder as Decoder>::Error> {
    let mut dec = T::Decoder::default();
    let v = dec.decode(buf)?.ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "buffer is incomplete")
    })?;
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
    use core::future::pending;
    use core::pin::Pin;
    use core::time::Duration;

    use futures::future::ready;
    use futures::stream;
    use tokio::sync::oneshot;

    use super::*;

    #[test_log::test(tokio::test)]
    async fn sync() -> anyhow::Result<()> {
        let mut buf = BytesMut::default();
        pack((0x42_u8, "foo".to_string()), &mut buf).await?;
        assert_eq!(buf.as_ref(), b"\x42\x03foo");
        let (a, b) = unpack::<(u8, String)>(&mut buf)?;
        assert_eq!(a, 0x42);
        assert_eq!(b, "foo");
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn future() -> anyhow::Result<()> {
        let (tx, rx) = oneshot::channel::<u32>();
        let fut: Pin<Box<dyn Future<Output = Pin<Box<dyn Future<Output = u32> + Send>>> + Send>> =
            Box::pin(ready(
                Box::pin(async { rx.await.unwrap() }) as Pin<Box<dyn Future<Output = u32> + Send>>
            ));
        let mut buf = BytesMut::default();
        let (res, ()) = tokio::join!(pack((fut, 0x01_u8), &mut buf), async {
            tx.send(0x42).unwrap();
        });
        res?;
        assert_eq!(buf.as_ref(), b"\x01\x01\x42\x01");

        let (fut, v) = unpack::<(
            Pin<Box<dyn Future<Output = Pin<Box<dyn Future<Output = u32> + Send>>> + Send>>,
            u8,
        )>(&mut buf)?;
        assert_eq!(v, 0x01);
        assert_eq!(fut.await.await, 0x42);
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn stream() -> anyhow::Result<()> {
        let s: Pin<Box<dyn Stream<Item = Vec<Pin<Box<dyn Future<Output = u8> + Send>>>> + Send>> =
            Box::pin(stream::iter([
                vec![
                    Box::pin(async { 1_u8 }) as Pin<Box<dyn Future<Output = u8> + Send>>,
                    Box::pin(async { 2 }),
                ],
                vec![],
                vec![Box::pin(async { 3 })],
            ]));
        let mut buf = BytesMut::default();
        pack(s, &mut buf).await?;
        assert_eq!(buf.as_ref(), b"\x01\x03\x01\x01\x01\x02\x01\x03");

        let s = unpack::<
            Pin<Box<dyn Stream<Item = Vec<Pin<Box<dyn Future<Output = u8> + Send>>>> + Send>>,
        >(&mut buf)?;
        let mut vs = vec![];
        for fut in s.concat().await {
            vs.push(fut.await);
        }
        assert_eq!(vs, [1, 2, 3]);
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn stream_bytes() -> anyhow::Result<()> {
        let s: Pin<Box<dyn Stream<Item = Bytes> + Send>> = Box::pin(stream::iter([
            Bytes::from_static(b"foo"),
            Bytes::from_static(b"bar"),
        ]));
        let r: Pin<Box<dyn AsyncRead + Send>> = Box::pin(std::io::Cursor::new(b"baz"));
        let mut buf = BytesMut::default();
        pack((s, r), &mut buf).await?;
        assert_eq!(buf.as_ref(), b"\x01\x06foobar\x01\x03baz");

        let (s, mut r) = unpack::<(
            Pin<Box<dyn Stream<Item = Bytes> + Send>>,
            Pin<Box<dyn AsyncRead + Send>>,
        )>(&mut buf)?;
        let s: Vec<Bytes> = s.collect().await;
        assert_eq!(s.concat(), b"foobar");
        let mut v = vec![];
        r.read_to_end(&mut v).await?;
        assert_eq!(v, b"baz");
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn stream_empty() -> anyhow::Result<()> {
        let s: Pin<Box<dyn Stream<Item = Vec<u8>> + Send>> = Box::pin(stream::empty());
        let b: Pin<Box<dyn Stream<Item = Bytes> + Send>> = Box::pin(stream::empty());
        let mut buf = BytesMut::default();
        pack((s, b, tokio::io::empty()), &mut buf).await?;
        assert_eq!(buf.as_ref(), b"\x01\x00\x01\x00\x01\x00");

        let (s, b, mut r) = unpack::<(
            Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>,
            Pin<Box<dyn Stream<Item = Bytes> + Send>>,
            Pin<Box<dyn AsyncRead + Send>>,
        )>(&mut buf)?;
        assert!(s.concat().await.is_empty());
        assert_eq!(b.count().await, 0);
        let mut v = vec![];
        r.read_to_end(&mut v).await?;
        assert!(v.is_empty());
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn pending_future() -> anyhow::Result<()> {
        let fut: Pin<Box<dyn Future<Output = u8> + Send>> = Box::pin(pending());
        let mut buf = BytesMut::default();
        let res = tokio::time::timeout(Duration::from_millis(10), pack(fut, &mut buf)).await;
        assert!(res.is_err());
        Ok(())
    }

    #[test]
    fn unpack_trailing() {
        let mut buf = BytesMut::from(&b"\x42\x00"[..]);
        let err = unpack::<u8>(&mut buf).err().unwrap();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test_log::test(tokio::test)]
    async fn unpack_pending() {
        let mut buf = BytesMut::from(&b"\x00"[..]);
        let err = unpack::<Pin<Box<dyn Future<Output = u8> + Send>>>(&mut buf)
            .err()
            .unwrap();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

        let mut buf = BytesMut::from(&b"\x00"[..]);
        let err = unpack::<Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>>(&mut buf)
            .err()
            .unwrap();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }
}
