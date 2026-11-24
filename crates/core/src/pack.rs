//! Packing and unpacking of wRPC values to/from singular, flat byte buffers

use bytes::BytesMut;
use tokio_util::codec::{Decoder, Encoder};

use crate::{Decode, Encode, Receive, Transmit};

/// Encodes `v` into `dst` using its default encoder, see [`pack_with`]
pub fn pack<T: Encode>(v: T, dst: &mut BytesMut) -> std::io::Result<()> {
    pack_with(T::Encoder::default(), v, dst)
}

/// Decodes a value [`pack`] encoded from `buf` using its default decoder, see [`unpack_with`]
pub fn unpack<T: Decode>(buf: &mut BytesMut) -> std::io::Result<T> {
    unpack_with(T::Decoder::default(), buf)
}

/// Encodes `v` into `dst` using `enc`, failing with [`WouldBlock`](std::io::ErrorKind::WouldBlock)
/// if it contains pending asynchronous values
///
/// The contents of `dst` are unspecified on failure.
pub fn pack_with<T>(
    mut enc: impl Encoder<T, Error = std::io::Error> + Transmit,
    v: T,
    dst: &mut BytesMut,
) -> std::io::Result<()> {
    enc.encode(v, dst)?;
    if !enc.is_complete() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "asynchronous value is pending",
        ));
    }
    Ok(())
}

/// Decodes a value [`pack_with`] encoded from `buf` using `dec`, failing if `buf` is incomplete,
/// contains pending asynchronous values or trailing bytes
///
/// The contents of `buf` are unspecified on failure.
pub fn unpack_with<D>(mut dec: D, buf: &mut BytesMut) -> std::io::Result<D::Item>
where
    D: Decoder<Error = std::io::Error> + Receive,
{
    let Some(v) = dec.decode(buf)? else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "buffer is incomplete",
        ));
    };
    if !dec.is_complete() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "buffer contains pending asynchronous values, which cannot be unpacked",
        ));
    }
    if !buf.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "buffer contains trailing bytes",
        ));
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use core::future::{pending, ready};

    use bytes::Bytes;
    use futures::future::{self, BoxFuture};
    use futures::stream::{self, BoxStream};
    use futures::{FutureExt as _, StreamExt as _};

    use super::*;

    #[test_log::test(tokio::test)]
    async fn roundtrip() -> anyhow::Result<()> {
        type Values = (
            u8,
            BoxFuture<'static, String>,
            BoxStream<'static, Vec<BoxFuture<'static, u8>>>,
            BoxStream<'static, Bytes>,
        );

        let v: Values = (
            0x42,
            ready("foo".into()).boxed(),
            stream::iter([
                vec![ready(1).boxed(), ready(2).boxed()],
                vec![],
                vec![ready(3).boxed()],
            ])
            .boxed(),
            stream::iter([Bytes::from("foo"), Bytes::from("bar")]).boxed(),
        );
        let mut buf = BytesMut::default();
        pack(v, &mut buf)?;
        let (a, b, c, d): Values = unpack(&mut buf)?;
        assert_eq!(a, 0x42);
        assert_eq!(b.await, "foo");
        assert_eq!(future::join_all(c.concat().await).await, [1, 2, 3]);
        let d: Vec<_> = d.collect().await;
        assert_eq!(d.concat(), b"foobar");
        Ok(())
    }

    #[test]
    fn rejects_invalid() {
        let fut = ready(pending::<u8>().boxed()).boxed();
        let err = pack(fut, &mut BytesMut::default()).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock);

        let err = unpack::<BoxFuture<'static, u8>>(&mut BytesMut::from("\0"))
            .err()
            .unwrap();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

        let err = unpack::<BoxStream<'static, Vec<u8>>>(&mut BytesMut::from("\0"))
            .err()
            .unwrap();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

        let err = unpack::<u8>(&mut BytesMut::from("\x42\0")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }
}
