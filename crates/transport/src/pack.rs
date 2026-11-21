//! Packing and unpacking of wRPC values to/from singular, flat byte buffers

use bytes::BytesMut;
use tokio_util::codec::{Decoder, Encoder};

use crate::{Receive, Transmit};

/// Packs `v` into a singular byte buffer `dst` using `enc`.
///
/// All asynchronous values contained in `v` must be resolved, this function returns an error of
/// kind [`std::io::ErrorKind::WouldBlock`] otherwise. Futures are polled exactly once and streams
/// are polled until they are pending or finished, so values backed by Tokio I/O types (e.g.
/// `tokio::fs::File`) are typically pending and require this function to be called within the
/// context of a Tokio runtime.
///
/// If this function returns an error, contents of `dst` are undefined.
pub fn pack<T>(
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

/// Unpacks a value from a byte buffer `buf` produced by [`pack`] using `dec`.
///
/// This function returns an error if `buf` contains pending asynchronous values or if `buf` is
/// not fully consumed.
///
/// If this function returns an error, contents of `buf` are undefined.
pub fn unpack<D>(mut dec: D, buf: &mut BytesMut) -> std::io::Result<D::Item>
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
    use crate::{Decode, Encode};

    #[test_log::test(tokio::test)]
    async fn roundtrip() -> anyhow::Result<()> {
        let v: (
            u8,
            BoxFuture<'static, String>,
            BoxStream<'static, Vec<BoxFuture<'static, u8>>>,
            BoxStream<'static, Bytes>,
        ) = (
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
        let enc = <(
            u8,
            BoxFuture<'static, String>,
            BoxStream<'static, Vec<BoxFuture<'static, u8>>>,
            BoxStream<'static, Bytes>,
        ) as Encode>::Encoder::default();
        pack(enc, v, &mut buf)?;
        let (a, b, c, d) = unpack(
            <(
                u8,
                BoxFuture<'static, String>,
                BoxStream<'static, Vec<BoxFuture<'static, u8>>>,
                BoxStream<'static, Bytes>,
            ) as Decode>::Decoder::default(),
            &mut buf,
        )?;
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
        let enc = <BoxFuture<'static, BoxFuture<'static, u8>> as Encode>::Encoder::default();
        let err = pack(enc, fut, &mut BytesMut::default()).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock);

        let err = unpack(
            <BoxFuture<'static, u8> as Decode>::Decoder::default(),
            &mut BytesMut::from("\0"),
        )
        .err()
        .unwrap();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

        let err = unpack(
            <BoxStream<'static, Vec<u8>> as Decode>::Decoder::default(),
            &mut BytesMut::from("\0"),
        )
        .err()
        .unwrap();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

        let err = unpack(
            <u8 as Decode>::Decoder::default(),
            &mut BytesMut::from("\x42\0"),
        )
        .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }
}
