//! Packing and unpacking of wRPC values to/from singular, flat byte buffers

use bytes::BytesMut;
use tokio_util::codec::{Decoder, Encoder};

use crate::{Decode, Deferred as _, Encode};

/// Pack a [`Encode`] into a singular byte buffer `dst`.
///
/// All asynchronous values contained in `v` must be ready, this function returns an error of
/// kind [`std::io::ErrorKind::WouldBlock`] if any of them is pending.
///
/// Futures are polled exactly once and streams are polled until they are pending or finished,
/// so values backed by Tokio I/O types (e.g. `tokio::fs::File`) are typically pending and require
/// this function to be called within the context of a Tokio runtime.
///
/// If this function returns an error, contents of `dst` are undefined.
pub fn pack<T: Encode>(v: T, dst: &mut BytesMut) -> Result<(), <T::Encoder as Encoder<T>>::Error> {
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

/// Unpack a [`Decode`] from a byte buffer `buf` produced by [`pack`].
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

    use bytes::Bytes;
    use futures::future::{self, BoxFuture};
    use futures::stream::{self, BoxStream};
    use futures::{FutureExt as _, StreamExt as _};
    use tokio::io::{AsyncRead, AsyncReadExt as _};

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

        let v: Value = (
            0x42,
            ready("foo".into()).boxed(),
            stream::iter((0..=255_u8).map(|i| vec![ready(i).boxed()])).boxed(),
            stream::iter([Bytes::from("foo"), Bytes::from("bar")]).boxed(),
            Box::pin(&b"foobar"[..]),
            vec![
                ready(Some(
                    stream::iter([vec!["foo".into()], vec![], vec!["bar".into()]]).boxed(),
                ))
                .boxed(),
                ready(None).boxed(),
            ],
            stream::empty().boxed(),
        );
        let mut buf = BytesMut::default();
        pack(v, &mut buf)?;
        let (a, b, c, d, mut e, f, g) = unpack::<Value>(&mut buf)?;
        assert_eq!(a, 0x42);
        assert_eq!(b.await, "foo");
        assert_eq!(
            future::join_all(c.concat().await).await,
            (0..=255).collect::<Vec<u8>>()
        );
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
