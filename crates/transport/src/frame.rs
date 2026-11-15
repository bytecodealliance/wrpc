//! wRPC transport stream framing
//!
//! Frames are read using a [`FramedRead`](tokio_util::codec::FramedRead) with a [Decoder]:
//!
//! ```
//! # async fn read(rx: impl tokio::io::AsyncRead + Unpin) -> std::io::Result<()> {
//! use futures::TryStreamExt as _;
//! use tokio_util::codec::FramedRead;
//! use wrpc_transport::FrameDecoder;
//!
//! let mut rx = FramedRead::new(rx, FrameDecoder::default());
//! while let Some(data) = rx.try_next().await? {
//!     let path = rx.decoder().path();
//!     println!("{path:?}: {data:?}");
//! }
//! # Ok(())
//! # }
//! ```

use bytes::BytesMut;
use tracing::{instrument, trace};
use wasm_tokio::{Leb128DecoderU32, Leb128Encoder};

pub use crate::mux::{Outgoing, Transmission};

/// Default maximum length of a frame path
pub(crate) const DEFAULT_MAX_DEPTH: u32 = 64;

/// wRPC frame reference
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameRef<'a> {
    /// Frame path
    pub path: &'a [usize],
    /// Frame data
    pub data: &'a [u8],
}

/// wRPC frame header reference
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameHeaderRef<'a> {
    /// Frame path
    pub path: &'a [usize],
    /// Length of the frame data
    pub size: usize,
}

/// Frame header decoder, which yields the length of the frame data and exposes the frame path
/// using [`FrameHeaderDecoder::path`]
#[derive(Clone, Debug)]
pub struct FrameHeaderDecoder {
    max_depth: u32,
    /// Length of the path being decoded, once decoded
    depth: Option<usize>,
    path: Vec<usize>,
}

impl FrameHeaderDecoder {
    /// Construct a new frame header decoder, which rejects paths longer than `max_depth`
    #[must_use]
    pub fn new(max_depth: u32) -> Self {
        Self {
            max_depth,
            depth: None,
            path: Vec::default(),
        }
    }

    /// Returns the path of the last decoded header, valid until `decode` is called again
    #[must_use]
    pub fn path(&self) -> &[usize] {
        &self.path
    }
}

impl Default for FrameHeaderDecoder {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_DEPTH)
    }
}

impl tokio_util::codec::Decoder for FrameHeaderDecoder {
    type Item = usize;
    type Error = std::io::Error;

    #[instrument(level = "trace", skip_all)]
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let depth = if let Some(depth) = self.depth {
            depth
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
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
            self.path.clear();
            self.depth = Some(n);
            n
        };
        while self.path.len() < depth {
            let Some(i) = Leb128DecoderU32.decode(src)? else {
                return Ok(None);
            };
            let i = usize::try_from(i)
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
            self.path.push(i);
        }
        let Some(n) = Leb128DecoderU32.decode(src)? else {
            return Ok(None);
        };
        trace!(path = ?self.path, n, "decoded data length");
        let n = usize::try_from(n)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        self.depth = None;
        Ok(Some(n))
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if let Some(n) = self.decode(src)? {
            return Ok(Some(n));
        }
        if self.depth.is_some() || !src.is_empty() {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        Ok(None)
    }
}

/// Frame decoder, which yields the data of each frame in chunks as soon as it is received, so
/// frames are never buffered, and exposes the frame path using [`Decoder::path`]
///
/// A frame with empty data, which closes its path, is yielded as a single empty chunk.
#[derive(Clone, Debug)]
pub struct Decoder {
    header: FrameHeaderDecoder,
    /// Length of the data left of the frame being decoded
    size: usize,
}

impl Decoder {
    /// Construct a new frame decoder, which rejects paths longer than `max_depth`
    #[must_use]
    pub fn new(max_depth: u32) -> Self {
        Self {
            header: FrameHeaderDecoder::new(max_depth),
            size: 0,
        }
    }

    /// Returns the path of the last decoded chunk, valid until `decode` is called again, for
    /// example, by polling a [`FramedRead`](tokio_util::codec::FramedRead) for the next one
    #[must_use]
    pub fn path(&self) -> &[usize] {
        self.header.path()
    }
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_DEPTH)
    }
}

impl tokio_util::codec::Decoder for Decoder {
    type Item = BytesMut;
    type Error = std::io::Error;

    #[instrument(level = "trace", skip_all)]
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if self.size == 0 {
            let Some(n) = self.header.decode(src)? else {
                return Ok(None);
            };
            if n == 0 {
                return Ok(Some(BytesMut::default()));
            }
            self.size = n;
        }
        if src.is_empty() {
            return Ok(None);
        }
        let data = src.split_to(self.size.min(src.len()));
        self.size = self.size.saturating_sub(data.len());
        trace!(path = ?self.path(), n = data.len(), "decoded data");
        Ok(Some(data))
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if let Some(data) = self.decode(src)? {
            return Ok(Some(data));
        }
        if self.size > 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        self.header.decode_eof(src)?;
        Ok(None)
    }
}

/// Frame header encoder
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameHeaderEncoder;

impl tokio_util::codec::Encoder<FrameHeaderRef<'_>> for FrameHeaderEncoder {
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

/// Frame encoder
#[derive(Clone, Copy, Debug, Default)]
pub struct Encoder;

impl tokio_util::codec::Encoder<FrameRef<'_>> for Encoder {
    type Error = std::io::Error;

    #[instrument(level = "trace", skip_all)]
    fn encode(
        &mut self,
        FrameRef { path, data }: FrameRef<'_>,
        dst: &mut BytesMut,
    ) -> Result<(), Self::Error> {
        FrameHeaderEncoder.encode(
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

#[cfg(test)]
mod tests {
    use futures::{SinkExt as _, TryStreamExt as _};
    use tokio_util::codec::{Decoder as _, Encoder as _, FramedRead, FramedWrite};

    use super::*;

    #[test_log::test(tokio::test)]
    async fn codec() -> std::io::Result<()> {
        let mut tx = FramedWrite::new(vec![], Encoder);

        tx.send(FrameRef {
            path: &[0, 1, 2],
            data: b"test",
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

        let data = rx.try_next().await?;
        assert_eq!(data.as_deref(), Some(b"test".as_slice()));
        assert_eq!(rx.decoder().path(), [0, 1, 2]);

        let data = rx.try_next().await?;
        assert_eq!(data.as_deref(), Some(b"".as_slice()));
        assert_eq!(rx.decoder().path(), []);

        let data = rx.try_next().await?;
        assert_eq!(data.as_deref(), Some("\x7fƒ𐍈Ő".as_bytes()));
        assert_eq!(rx.decoder().path(), [0x42]);

        let data = rx.try_next().await.expect("failed to get EOF");
        assert_eq!(data, None);

        Ok(())
    }

    #[test]
    fn split_frame() {
        let mut buf = BytesMut::default();
        for frame in [
            FrameRef {
                path: &[1, 2],
                data: b"test",
            },
            FrameRef {
                path: &[3],
                data: b"",
            },
        ] {
            Encoder.encode(frame, &mut buf).unwrap();
        }
        let mut dec = Decoder::default();
        let mut src = buf.split_to(5);
        let data = dec.decode(&mut src).unwrap();
        assert_eq!(data.as_deref(), Some(b"t".as_slice()));
        assert_eq!(dec.path(), [1, 2]);
        src.unsplit(buf.split_to(4));
        let data = dec.decode(&mut src).unwrap();
        assert_eq!(data.as_deref(), Some(b"est".as_slice()));
        assert_eq!(dec.path(), [1, 2]);
        assert_eq!(dec.decode(&mut src).unwrap(), None);
        src.unsplit(buf);
        let data = dec.decode(&mut src).unwrap();
        assert_eq!(data.as_deref(), Some(b"".as_slice()));
        assert_eq!(dec.path(), [3]);
        assert!(src.is_empty());
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

        let mut rx = FramedRead::new(b"\x01\x80".as_slice(), Decoder::default());
        let err = rx.try_next().await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
    }
}
