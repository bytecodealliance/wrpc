use std::sync::Arc;

use bytes::{Buf as _, BufMut as _, BytesMut};
use tokio_util::codec::Decoder as _;
use tracing::{instrument, trace};
use wasm_tokio::{DEFAULT_MAX_INITIAL_CAPACITY, Leb128DecoderU32, Leb128Encoder};

use crate::frame::{Frame, FrameRef, Header, HeaderReadError, MAX_INITIAL_PATH_CAPACITY, PROTOCOL};

/// Default maximum length of a [Frame] path
const DEFAULT_MAX_DEPTH: u32 = 64;

/// Default maximum length of the data of a [Frame]
const DEFAULT_MAX_SIZE: u32 = 16 << 20;

/// Default maximum length of the instance and function names in a [Header]
const DEFAULT_MAX_NAME_LEN: u32 = 1 << 10;

/// Decodes a LEB128-encoded [u32] from `src`, consuming all of `src` and keeping the bytes of an
/// incomplete one in `buf` if `src` ends before it does
fn decode_u32(buf: &mut BytesMut, src: &mut BytesMut) -> std::io::Result<Option<u32>> {
    if buf.is_empty() {
        if let Some(n) = Leb128DecoderU32.decode(src)? {
            return Ok(Some(n));
        }
        buf.extend_from_slice(src);
        src.clear();
        return Ok(None);
    }
    while src.has_remaining() {
        buf.put_u8(src.get_u8());
        if let Some(n) = Leb128DecoderU32.decode(buf)? {
            return Ok(Some(n));
        }
    }
    Ok(None)
}

/// [Header] decoder, which consumes all bytes it is given up to the end of the [Header]
#[derive(Clone, Debug)]
pub struct HeaderDecoder {
    max_instance_len: u32,
    max_name_len: u32,
    /// Whether the version byte was decoded
    version: bool,
    /// Bytes of the name length being decoded
    len: BytesMut,
    /// Length and bytes of the name being decoded, once its length is decoded
    name: Option<(usize, Vec<u8>)>,
    /// Instance name, once decoded
    instance: Option<String>,
}

impl HeaderDecoder {
    /// Construct a new [Header] decoder, which rejects instance and function names longer than
    /// `max_instance_len` and `max_name_len` bytes respectively
    #[must_use]
    pub fn new(max_instance_len: u32, max_name_len: u32) -> Self {
        Self {
            max_instance_len,
            max_name_len,
            version: false,
            len: BytesMut::default(),
            name: Option::default(),
            instance: Option::default(),
        }
    }

    /// Returns the maximum length of an encoded [Header]
    pub(crate) fn max_len(&self) -> usize {
        let instance = usize::try_from(self.max_instance_len).unwrap_or(usize::MAX);
        let name = usize::try_from(self.max_name_len).unwrap_or(usize::MAX);
        instance.saturating_add(name).saturating_add(1 + 5 + 5)
    }

    /// Decodes a name of at most `max_len` bytes
    fn decode_name(
        &mut self,
        src: &mut BytesMut,
        max_len: u32,
        kind: &str,
    ) -> std::io::Result<Option<String>> {
        let (n, mut buf) = if let Some(name) = self.name.take() {
            name
        } else {
            let Some(n) = decode_u32(&mut self.len, src)? else {
                return Ok(None);
            };
            if n > max_len {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("{kind} name length of `{n}` exceeds maximum of `{max_len}`"),
                ));
            }
            let n = usize::try_from(n)
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
            (n, Vec::with_capacity(n.min(DEFAULT_MAX_INITIAL_CAPACITY)))
        };
        let k = n.saturating_sub(buf.len()).min(src.len());
        buf.extend_from_slice(&src[..k]);
        src.advance(k);
        if buf.len() < n {
            self.name = Some((n, buf));
            return Ok(None);
        }
        String::from_utf8(buf)
            .map(Some)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))
    }
}

impl Default for HeaderDecoder {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_NAME_LEN, DEFAULT_MAX_NAME_LEN)
    }
}

impl tokio_util::codec::Decoder for HeaderDecoder {
    type Item = Header;
    type Error = HeaderReadError;

    #[instrument(level = "trace", skip_all)]
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if !self.version {
            let Some(v) = src.first().copied() else {
                return Ok(None);
            };
            if v != PROTOCOL {
                return Err(HeaderReadError::UnsupportedVersion(v));
            }
            src.advance(1);
            self.version = true;
        }
        let instance = if let Some(instance) = self.instance.take() {
            instance
        } else {
            let Some(instance) = self.decode_name(src, self.max_instance_len, "instance")? else {
                return Ok(None);
            };
            instance
        };
        let Some(name) = self.decode_name(src, self.max_name_len, "function")? else {
            self.instance = Some(instance);
            return Ok(None);
        };
        self.version = false;
        trace!(instance, name, "decoded header");
        Ok(Some(Header { instance, name }))
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if let Some(header) = self.decode(src)? {
            return Ok(Some(header));
        }
        if self.version {
            return Err(HeaderReadError::IO(
                std::io::ErrorKind::UnexpectedEof.into(),
            ));
        }
        Ok(None)
    }
}

/// [Frame] header decoder, which decodes the path and the data length of a [Frame], consuming
/// all bytes it is given up to the end of the header
#[derive(Debug)]
pub(crate) struct FrameHeaderDecoder {
    /// Bytes of the integer being decoded
    buf: BytesMut,
    /// Path, once its length is decoded
    path: Option<Vec<usize>>,
    /// Number of path elements left to decode
    path_cap: usize,
    max_depth: u32,
    max_size: u32,
}

impl FrameHeaderDecoder {
    /// Construct a new [Frame] header decoder, which rejects paths longer than `max_depth` and
    /// data longer than `max_size` bytes
    pub(crate) fn new(max_depth: u32, max_size: u32) -> Self {
        Self {
            buf: BytesMut::default(),
            path: Option::default(),
            path_cap: 0,
            max_depth,
            max_size,
        }
    }

    /// Returns the maximum length of an encoded [Frame] header
    pub(crate) fn max_len(&self) -> usize {
        let depth = usize::try_from(self.max_depth).unwrap_or(usize::MAX);
        depth.saturating_add(2).saturating_mul(5)
    }
}

impl tokio_util::codec::Decoder for FrameHeaderDecoder {
    type Item = (Vec<usize>, usize);
    type Error = std::io::Error;

    #[instrument(level = "trace", skip_all)]
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let mut path = if let Some(path) = self.path.take() {
            path
        } else {
            trace!("decoding path length");
            let Some(n) = decode_u32(&mut self.buf, src)? else {
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
            self.path_cap = n;
            Vec::with_capacity(n.min(MAX_INITIAL_PATH_CAPACITY))
        };
        while self.path_cap > 0 {
            trace!(self.path_cap, "decoding path element");
            let Some(i) = decode_u32(&mut self.buf, src)? else {
                self.path = Some(path);
                return Ok(None);
            };
            trace!(i, "decoded path element");
            let i = usize::try_from(i)
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
            path.push(i);
            self.path_cap = self.path_cap.saturating_sub(1);
        }
        trace!("decoding data length");
        let Some(n) = decode_u32(&mut self.buf, src)? else {
            self.path = Some(path);
            return Ok(None);
        };
        trace!(n, "decoded data length");
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
        Ok(Some((path, n)))
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if let Some(header) = self.decode(src)? {
            return Ok(Some(header));
        }
        if self.path.is_some() || !self.buf.is_empty() {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        Ok(None)
    }
}

/// [Frame] decoder
pub struct Decoder {
    header: FrameHeaderDecoder,
    /// Path and data length of the frame being decoded, once its header is decoded
    frame: Option<(Vec<usize>, usize)>,
}

impl Decoder {
    /// Construct a new [Frame] decoder
    #[must_use]
    pub fn new(max_depth: u32, max_size: u32) -> Self {
        Self {
            header: FrameHeaderDecoder::new(max_depth, max_size),
            frame: Option::default(),
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
        let (path, n) = if let Some(frame) = self.frame.take() {
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
            self.frame = Some((path, n));
            return Ok(None);
        }
        trace!(n, "decoding data");
        let data = src.split_to(n).freeze();
        Ok(Some(Frame {
            path: Arc::from(path),
            data,
        }))
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
        let size = data.len();
        let depth = path.len();
        dst.reserve(size.saturating_add(depth).saturating_add(5 + 5));
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
        Leb128Encoder.encode(n, dst)?;
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
    async fn decoder_limits() {
        let mut buf = BytesMut::default();
        crate::frame::encode_invocation(&mut buf, "foo", "bar", 1).unwrap();
        let Err(HeaderReadError::IO(err)) = HeaderDecoder::new(2, 3).decode(&mut buf.clone())
        else {
            panic!("decoding a header with a too long instance name should fail");
        };
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        let mut dec = HeaderDecoder::default();
        let mut header = None;
        while header.is_none() {
            let mut b = buf.split_to(1);
            header = dec.decode(&mut b).unwrap();
            assert!(b.is_empty());
        }
        assert_eq!(
            header,
            Some(Header {
                instance: "foo".into(),
                name: "bar".into(),
            })
        );
        assert_eq!(buf, b"\0\x01".as_slice());

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
}
