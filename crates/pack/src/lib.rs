//! Stopgap solution for packing and unpacking wRPC values to/from singular, flat byte buffers.
//!
//! All APIs in this crate are to be considered unstable and everything may break arbitrarily.
//!
//! This crate will never reach 1.0 and will be deprecated once <https://github.com/bytecodealliance/wrpc/issues/25> is complete
//!
//! This crate is maintained on a best-effort basis.

use bytes::BytesMut;
use tokio_util::codec::{Decoder, Encoder};
use wrpc_transport::{Decode, Deferred as _, Encode};

/// Pack a [`wrpc_transport::Encode`] into a singular byte buffer `dst`.
///
/// This function does not support asynchronous values and will return an error is such a value is
/// passed in.
///
/// This is unstable API, which will be deprecated once feature-complete "packing" functionality is available in [`wrpc_transport`].
/// Track <https://github.com/bytecodealliance/wrpc/issues/25> for updates.
pub fn pack<T: Encode>(v: T, dst: &mut BytesMut) -> Result<(), <T::Encoder as Encoder<T>>::Error> {
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
pub fn unpack<T: Decode>(buf: &mut BytesMut) -> Result<T, <T::Decoder as Decoder>::Error> {
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
