//! Stopgap solution for packing and unpacking wRPC values to/from singular, flat byte buffers.
//!
//! All APIs in this crate are to be considered unstable and everything may break arbitrarily.
//!
//! This crate will never reach 1.0 and will be deprecated once <https://github.com/bytecodealliance/wrpc/issues/25> is complete
//!
//! This crate is maintained on a best-effort basis.

use core::pin::Pin;
use core::task::{Context, Poll};

use bytes::BytesMut;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Decoder, Encoder};
use wrpc_transport::{Decode, Deferred as _, Encode};

/// A stream, which fails on each operation, this type should only ever be used in trait bounds
pub struct NoopStream;

impl AsyncRead for NoopStream {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Err(std::io::Error::other("should not be called")))
    }
}

impl AsyncWrite for NoopStream {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(Err(std::io::Error::other("should not be called")))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Err(std::io::Error::other("should not be called")))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Err(std::io::Error::other("should not be called")))
    }
}

impl wrpc_transport::Index<Self> for NoopStream {
    fn index(&self, _: &[usize]) -> anyhow::Result<Self> {
        anyhow::bail!("should not be called")
    }
}

/// Pack a [`wrpc_transport::Encode`] into a singular byte buffer `dst`.
///
/// This function does not support asynchronous values and will return an error is such a value is
/// passed in.
///
/// This is unstable API, which will be deprecated once feature-complete "packing" functionality is available in [`wrpc_transport`].
/// Track <https://github.com/bytecodealliance/wrpc/issues/25> for updates.
pub fn pack<T: Encode<NoopStream>>(
    v: T,
    dst: &mut BytesMut,
) -> Result<(), <T::Encoder as Encoder<T>>::Error> {
    let mut enc = T::Encoder::default();
    enc.encode(v, dst)?;
    if enc.take_deferred().is_some() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "value contains pending asynchronous values and cannot be packed",
        )
        .into());
    }
    Ok(())
}

/// Unpack a [`wrpc_transport::Decode`] from a byte buffer `dst`.
///
/// This function does not support asynchronous values and will return an error if `buf` contains pending async values.
///
/// If this function returns an error, contents of `buf` are undefined.
///
/// This is unstable API, which will be deprecated once feature-complete "unpacking" functionality is available in [`wrpc_transport`].
/// Track <https://github.com/bytecodealliance/wrpc/issues/25> for updates.
pub fn unpack<T: Decode<NoopStream>>(
    buf: &mut BytesMut,
) -> Result<T, <T::Decoder as Decoder>::Error> {
    let mut dec = T::Decoder::default();
    let v = dec.decode(buf)?;
    let v = v.ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "buffer is incomplete")
    })?;
    if dec.take_deferred().is_some() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "buffer contains pending asynchronous values and cannot be unpacked",
        )
        .into());
    }
    Ok(v)
}
