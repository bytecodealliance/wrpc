//! Packing and unpacking of wRPC values to/from singular, flat byte buffers

use core::error::Error;
use core::fmt;

use bytes::BytesMut;
use tokio_util::codec::{Decoder, Encoder};

use crate::{Decode, Encode, Receive, Transmit};

/// Error of [packing](pack_with) a value
#[derive(Debug)]
pub enum PackError {
    /// The encoder failed
    Encode(std::io::Error),
    /// The value contains pending asynchronous values
    Pending,
}

impl fmt::Display for PackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encode(..) => f.write_str("failed to encode value"),
            Self::Pending => f.write_str("asynchronous value is pending"),
        }
    }
}

impl Error for PackError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Encode(err) => Some(err),
            Self::Pending => None,
        }
    }
}

impl From<PackError> for std::io::Error {
    fn from(err: PackError) -> Self {
        match err {
            PackError::Encode(err) => err,
            PackError::Pending => Self::new(std::io::ErrorKind::WouldBlock, err),
        }
    }
}

/// Error of [unpacking](unpack_with) a value
#[derive(Debug)]
pub enum UnpackError {
    /// The decoder failed
    Decode(std::io::Error),
    /// The buffer ended before the value
    Incomplete,
    /// The buffer contains pending asynchronous values
    Pending,
    /// The buffer contains bytes after the value
    TrailingBytes,
}

impl fmt::Display for UnpackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode(..) => f.write_str("failed to decode value"),
            Self::Incomplete => f.write_str("buffer is incomplete"),
            Self::Pending => f.write_str("buffer contains pending asynchronous values"),
            Self::TrailingBytes => f.write_str("buffer contains trailing bytes"),
        }
    }
}

impl Error for UnpackError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Decode(err) => Some(err),
            Self::Incomplete | Self::Pending | Self::TrailingBytes => None,
        }
    }
}

impl From<UnpackError> for std::io::Error {
    fn from(err: UnpackError) -> Self {
        match err {
            UnpackError::Decode(err) => err,
            UnpackError::Incomplete => Self::new(std::io::ErrorKind::UnexpectedEof, err),
            UnpackError::Pending | UnpackError::TrailingBytes => {
                Self::new(std::io::ErrorKind::InvalidData, err)
            }
        }
    }
}

/// Encodes `v` into `dst` using its default encoder, see [`pack_with`]
pub fn pack<T: Encode>(v: T, dst: &mut BytesMut) -> Result<(), PackError> {
    pack_with(T::Encoder::default(), v, dst)
}

/// Decodes a value [`pack`] encoded from `buf` using its default decoder, see [`unpack_with`]
pub fn unpack<T: Decode>(buf: impl Into<BytesMut>) -> Result<T, UnpackError> {
    unpack_with(T::Decoder::default(), buf)
}

/// Encodes `v` into `dst` using `enc`, failing with [`PackError::Pending`] if it contains pending
/// asynchronous values
///
/// The contents of `dst` are unspecified on failure.
pub fn pack_with<T>(
    mut enc: impl Encoder<T, Error = std::io::Error> + Transmit,
    v: T,
    dst: &mut BytesMut,
) -> Result<(), PackError> {
    enc.encode(v, dst).map_err(PackError::Encode)?;
    if !enc.is_complete() {
        return Err(PackError::Pending);
    }
    Ok(())
}

/// Decodes a value [`pack_with`] encoded from `buf` using `dec`, failing with
/// [`UnpackError::Incomplete`], [`UnpackError::Pending`] or [`UnpackError::TrailingBytes`] if `buf`
/// is incomplete, contains pending asynchronous values or trailing bytes
///
/// `buf` is copied once if it is borrowed, such as `&[u8]`, or a [`Bytes`](bytes::Bytes) that is
/// not [unique](bytes::Bytes::is_unique).
pub fn unpack_with<D>(mut dec: D, buf: impl Into<BytesMut>) -> Result<D::Item, UnpackError>
where
    D: Decoder<Error = std::io::Error> + Receive,
{
    let mut buf = buf.into();
    let Some(v) = dec.decode(&mut buf).map_err(UnpackError::Decode)? else {
        return Err(UnpackError::Incomplete);
    };
    if !dec.is_complete() {
        return Err(UnpackError::Pending);
    }
    if !buf.is_empty() {
        return Err(UnpackError::TrailingBytes);
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
        let (a, b, c, d): Values = unpack(buf)?;
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
        assert!(matches!(err, PackError::Pending), "{err:?}");

        let err = unpack::<BoxFuture<'static, u8>>("\0").err().unwrap();
        assert!(matches!(err, UnpackError::Pending), "{err:?}");

        let err = unpack::<BoxStream<'static, Vec<u8>>>("\0").err().unwrap();
        assert!(matches!(err, UnpackError::Pending), "{err:?}");

        let err = unpack::<u8>("\x42\0").unwrap_err();
        assert!(matches!(err, UnpackError::TrailingBytes), "{err:?}");

        let err = unpack::<u32>(&b"\x80"[..]).unwrap_err();
        assert!(matches!(err, UnpackError::Incomplete), "{err:?}");

        let err = unpack::<bool>("\x02").unwrap_err();
        assert!(
            matches!(&err, UnpackError::Decode(err) if err.kind() == std::io::ErrorKind::InvalidData),
            "{err:?}"
        );
    }
}
