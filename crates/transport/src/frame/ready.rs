//! Framing of values without pending asynchronous values

use bytes::BytesMut;
use futures::TryStreamExt as _;
use tokio::io::AsyncRead;
use tokio_util::codec::{Encoder as _, FramedRead};

use crate::frame::{Decoder, Encoder, FrameRef};

/// Reads a framed stream from `rx`, which carries no pending asynchronous values, and returns the
/// data received on the root path.
///
/// This is the counterpart of [`encode_ready`] and the returned buffer can be decoded using
/// [`unpack`](crate::unpack).
///
/// # Errors
///
/// Returns an error of kind [`std::io::ErrorKind::InvalidData`] if a frame is received on a nested
/// path or after the root path was closed, as well as any error returned by `rx`.
pub async fn read_ready(rx: impl AsyncRead + Unpin) -> std::io::Result<BytesMut> {
    let mut frames = FramedRead::new(rx, Decoder::default());
    let mut buf = BytesMut::default();
    while let Some(frame) = frames.try_next().await? {
        if !frame.path.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "received a frame on path `{:?}`, pending asynchronous values are not supported",
                    frame.path
                ),
            ));
        }
        if frame.data.is_empty() {
            if frames.try_next().await?.is_some() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "received a frame after the root path was closed",
                ));
            }
            break;
        }
        buf.extend_from_slice(&frame.data);
    }
    Ok(buf)
}

/// Encodes `data` as a framed stream on the root path into `dst`.
///
/// `data` must not contain pending asynchronous values, e.g. it can be produced by
/// [`pack`](crate::pack) or [`pack_ready`](crate::pack_ready).
/// This is the counterpart of [`read_ready`].
///
/// # Errors
///
/// Returns an error if `data` is too large to be encoded in a frame.
pub fn encode_ready(data: &[u8], dst: &mut BytesMut) -> std::io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    Encoder.encode(FrameRef { path: &[], data }, dst)
}

#[cfg(test)]
mod tests {
    use crate::{pack_ready, unpack};

    use super::*;

    #[test_log::test(tokio::test)]
    async fn roundtrip() -> anyhow::Result<()> {
        let mut data = BytesMut::default();
        pack_ready((0x42_u8, "foo"), &mut data)?;
        let mut buf = BytesMut::default();
        encode_ready(&data, &mut buf)?;
        encode_ready(&[], &mut buf)?;
        assert_eq!(buf.as_ref(), b"\x00\x05\x42\x03foo");

        let mut data = read_ready(buf.as_ref()).await?;
        let (a, b) = unpack::<(u8, String)>(&mut data)?;
        assert_eq!(a, 0x42);
        assert_eq!(b, "foo");
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn split() -> anyhow::Result<()> {
        let buf = b"\x00\x02\x42\x03\x00\x03foo\x00\x00";
        let mut data = read_ready(buf.as_slice()).await?;
        let (a, b) = unpack::<(u8, String)>(&mut data)?;
        assert_eq!(a, 0x42);
        assert_eq!(b, "foo");
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn nested() {
        let buf = b"\x00\x01\x00\x01\x00\x01\x42";
        let err = read_ready(buf.as_slice()).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test_log::test(tokio::test)]
    async fn closed() {
        let buf = b"\x00\x01\x42\x00\x00\x00\x01\x42";
        let err = read_ready(buf.as_slice()).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test_log::test(tokio::test)]
    async fn truncated() {
        let buf = b"\x00\x02\x42";
        assert!(read_ready(buf.as_slice()).await.is_err());
    }
}
