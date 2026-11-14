//! wRPC transport stream framing

use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use tracing::{instrument, trace};
use wasm_tokio::{DEFAULT_MAX_INITIAL_CAPACITY, Leb128DecoderU32, Leb128Encoder};

pub use crate::mux::{Closed, Incoming, Outgoing};

/// Default maximum length of a [Frame] path
const DEFAULT_MAX_DEPTH: u32 = 64;

/// Default maximum length of the data of a [Frame]
const DEFAULT_MAX_SIZE: u32 = 16 << 20;

const MAX_INITIAL_PATH_CAPACITY: usize = 32;

/// Owned wRPC frame
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Frame {
    /// Frame path
    pub path: Arc<[usize]>,
    /// Frame data
    pub data: Bytes,
}

/// wRPC frame reference
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameRef<'a> {
    /// Frame path
    pub path: &'a [usize],
    /// Frame data
    pub data: &'a [u8],
}

impl<'a> From<&'a Frame> for FrameRef<'a> {
    fn from(Frame { path, data }: &'a Frame) -> Self {
        Self {
            path: path.as_ref(),
            data: data.as_ref(),
        }
    }
}

/// Owned wRPC frame header
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameHeader {
    /// Frame path
    pub path: Arc<[usize]>,
    /// Length of the frame data
    pub size: usize,
}

/// wRPC frame header reference
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameHeaderRef<'a> {
    /// Frame path
    pub path: &'a [usize],
    /// Length of the frame data
    pub size: usize,
}

impl<'a> From<&'a FrameHeader> for FrameHeaderRef<'a> {
    fn from(FrameHeader { path, size }: &'a FrameHeader) -> Self {
        Self { path, size: *size }
    }
}

/// [Frame] header decoder, which decodes the path and the data length of a [Frame]
#[derive(Clone, Debug)]
pub struct HeaderDecoder {
    max_depth: u32,
    max_size: u32,
    /// Length and decoded elements of the path, once its length is decoded
    path: Option<(usize, Vec<usize>)>,
}

impl HeaderDecoder {
    /// Construct a new [Frame] header decoder, which rejects paths longer than `max_depth` and
    /// data longer than `max_size` bytes
    #[must_use]
    pub fn new(max_depth: u32, max_size: u32) -> Self {
        Self {
            max_depth,
            max_size,
            path: None,
        }
    }

    /// Returns the minimum number of bytes, which complete the header once [`decode`] returned
    /// [None]
    ///
    /// [`decode`]: tokio_util::codec::Decoder::decode
    #[must_use]
    pub fn min_remaining(&self) -> usize {
        let Some((depth, path)) = &self.path else {
            return 2;
        };
        depth.saturating_sub(path.len()).saturating_add(1)
    }
}

impl tokio_util::codec::Decoder for HeaderDecoder {
    type Item = FrameHeader;
    type Error = std::io::Error;

    #[instrument(level = "trace", skip_all)]
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let (depth, mut path) = if let Some(path) = self.path.take() {
            path
        } else {
            let Some(n) = Leb128DecoderU32.decode(src)? else {
                return Ok(None);
            };
            trace!(n, "decoded path length");
            if n > self.max_depth {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "path length of `{n}` exceeds maximum of `{}`",
                        self.max_depth
                    ),
                ));
            }
            let n = usize::try_from(n)
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
            (n, Vec::with_capacity(n.min(MAX_INITIAL_PATH_CAPACITY)))
        };
        while path.len() < depth {
            let Some(i) = Leb128DecoderU32.decode(src)? else {
                self.path = Some((depth, path));
                return Ok(None);
            };
            let i = usize::try_from(i)
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
            path.push(i);
        }
        let Some(n) = Leb128DecoderU32.decode(src)? else {
            self.path = Some((depth, path));
            return Ok(None);
        };
        trace!(?path, n, "decoded data length");
        if n > self.max_size {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "payload length of `{n}` exceeds maximum of `{}`",
                    self.max_size
                ),
            ));
        }
        let n = usize::try_from(n)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
        Ok(Some(FrameHeader {
            path: path.into(),
            size: n,
        }))
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if let Some(header) = self.decode(src)? {
            return Ok(Some(header));
        }
        if self.path.is_some() || !src.is_empty() {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        Ok(None)
    }
}

/// [Frame] decoder
pub struct Decoder {
    header: HeaderDecoder,
    /// Path and data length of the frame being decoded, once its header is decoded
    frame: Option<FrameHeader>,
}

impl Decoder {
    /// Construct a new [Frame] decoder
    #[must_use]
    pub fn new(max_depth: u32, max_size: u32) -> Self {
        Self {
            header: HeaderDecoder::new(max_depth, max_size),
            frame: None,
        }
    }
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_DEPTH, DEFAULT_MAX_SIZE)
    }
}

impl tokio_util::codec::Decoder for Decoder {
    type Item = Frame;
    type Error = std::io::Error;

    #[instrument(level = "trace", skip_all)]
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let FrameHeader { path, size: n } = if let Some(frame) = self.frame.take() {
            frame
        } else {
            let Some(header) = self.header.decode(src)? else {
                return Ok(None);
            };
            header
        };
        if src.len() < n {
            src.reserve(
                n.saturating_sub(src.len())
                    .min(DEFAULT_MAX_INITIAL_CAPACITY),
            );
            self.frame = Some(FrameHeader { path, size: n });
            return Ok(None);
        }
        trace!(n, "decoding data");
        let data = src.split_to(n).freeze();
        Ok(Some(Frame { path, data }))
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if let Some(frame) = self.decode(src)? {
            return Ok(Some(frame));
        }
        if self.frame.is_some() {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        self.header.decode_eof(src)?;
        Ok(None)
    }
}

/// [Frame] header encoder, which encodes the path and the data length of a [Frame]
#[derive(Clone, Copy, Debug, Default)]
pub struct HeaderEncoder;

impl tokio_util::codec::Encoder<FrameHeaderRef<'_>> for HeaderEncoder {
    type Error = std::io::Error;

    #[instrument(level = "trace", skip_all)]
    fn encode(
        &mut self,
        FrameHeaderRef { path, size }: FrameHeaderRef<'_>,
        dst: &mut BytesMut,
    ) -> Result<(), Self::Error> {
        let depth = path.len();
        dst.reserve(depth.saturating_add(5 + 5));
        let n = u32::try_from(depth)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
        trace!(n, "encoding path length");
        Leb128Encoder.encode(n, dst)?;
        for p in path {
            let p = u32::try_from(*p)
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
            trace!(p, "encoding path element");
            Leb128Encoder.encode(p, dst)?;
        }
        let n = u32::try_from(size)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
        trace!(n, "encoding data length");
        Leb128Encoder.encode(n, dst)
    }
}

impl tokio_util::codec::Encoder<&FrameHeader> for HeaderEncoder {
    type Error = std::io::Error;

    #[instrument(level = "trace", skip_all)]
    fn encode(&mut self, header: &FrameHeader, dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.encode(FrameHeaderRef::from(header), dst)
    }
}

/// [Frame] encoder
pub struct Encoder;

impl tokio_util::codec::Encoder<FrameRef<'_>> for Encoder {
    type Error = std::io::Error;

    #[instrument(level = "trace", skip_all)]
    fn encode(
        &mut self,
        FrameRef { path, data }: FrameRef<'_>,
        dst: &mut BytesMut,
    ) -> Result<(), Self::Error> {
        HeaderEncoder.encode(
            FrameHeaderRef {
                path,
                size: data.len(),
            },
            dst,
        )?;
        dst.extend_from_slice(data);
        Ok(())
    }
}

impl tokio_util::codec::Encoder<&Frame> for Encoder {
    type Error = std::io::Error;

    #[instrument(level = "trace", skip_all)]
    fn encode(&mut self, frame: &Frame, dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.encode(FrameRef::from(frame), dst)
    }
}

#[cfg(test)]
mod tests {
    use futures::{SinkExt as _, TryStreamExt as _};
    use tokio_util::codec::{FramedRead, FramedWrite};

    use super::*;

    #[test_log::test(tokio::test)]
    async fn codec() -> std::io::Result<()> {
        let mut tx = FramedWrite::new(vec![], Encoder);

        tx.send(&Frame {
            path: [0, 1, 2].into(),
            data: "test".into(),
        })
        .await?;

        tx.send(FrameRef {
            path: &[],
            data: b"",
        })
        .await?;

        tx.send(FrameRef {
            path: &[0x42],
            data: "\x7fƒ𐍈Ő".as_bytes(),
        })
        .await?;

        let tx = tx.into_inner();
        assert_eq!(
            tx,
            concat!(
                concat!("\x03", concat!("\0", "\x01", "\x02"), "\x04test"),
                concat!("\0", "\0"),
                concat!("\x01", concat!("\x42"), "\x09\x7fƒ𐍈Ő"),
            )
            .as_bytes()
        );

        let mut rx = FramedRead::new(tx.as_slice(), Decoder::default());

        let s = rx.try_next().await?;
        assert_eq!(
            s,
            Some(Frame {
                path: [0, 1, 2].into(),
                data: "test".into(),
            })
        );

        let s = rx.try_next().await?;
        assert_eq!(
            s,
            Some(Frame {
                path: [].into(),
                data: "".into(),
            })
        );

        let s = rx.try_next().await?;
        assert_eq!(
            s,
            Some(Frame {
                path: [0x42].into(),
                data: "\x7fƒ𐍈Ő".into(),
            })
        );

        let s = rx.try_next().await.expect("failed to get EOF");
        assert_eq!(s, None);

        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn frame_limits() {
        let mut tx = FramedWrite::new(vec![], Encoder);
        tx.send(FrameRef {
            path: &[0; DEFAULT_MAX_DEPTH as usize + 1],
            data: b"",
        })
        .await
        .unwrap();
        let tx = tx.into_inner();
        let mut rx = FramedRead::new(tx.as_slice(), Decoder::default());
        let err = rx.try_next().await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

        let mut rx = FramedRead::new(b"\0\xff\xff\xff\xff\x0f".as_slice(), Decoder::default());
        let err = rx.try_next().await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

        let mut rx = FramedRead::new(b"\x01\x80".as_slice(), Decoder::default());
        let err = rx.try_next().await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
    }
    #[test]
    fn header_min_remaining() {
        use tokio_util::codec::Decoder as _;

        let mut dec = HeaderDecoder::new(DEFAULT_MAX_DEPTH, DEFAULT_MAX_SIZE);
        assert_eq!(dec.min_remaining(), 2);
        let mut buf = BytesMut::from(&b"\x02\x80"[..]);
        assert_eq!(dec.decode(&mut buf).unwrap(), None);
        assert_eq!(dec.min_remaining(), 3);
        buf.extend_from_slice(b"\x01\x2a\x00");
        let header = dec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(*header.path, [128, 42]);
        assert_eq!(dec.min_remaining(), 2);
    }
}
