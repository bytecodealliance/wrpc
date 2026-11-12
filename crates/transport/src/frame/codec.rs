use std::sync::Arc;

use bytes::{Buf as _, Bytes, BytesMut};
use tracing::{instrument, trace};
use wasm_tokio::{CoreNameDecoder, DEFAULT_MAX_INITIAL_CAPACITY, Leb128DecoderU32, Leb128Encoder};

use crate::frame::{
    DEFAULT_MAX_NAME_LEN, Frame, FrameRef, Header, HeaderReadError, MAX_INITIAL_PATH_CAPACITY,
    PROTOCOL,
};

/// Default maximum length of a [Frame] path
const DEFAULT_MAX_DEPTH: u32 = 64;

/// Default maximum length of the data of a [Frame]
const DEFAULT_MAX_SIZE: u32 = 16 << 20;

/// [Header] decoder
#[derive(Debug)]
pub struct HeaderDecoder {
    instance_dec: CoreNameDecoder,
    name_dec: CoreNameDecoder,
    version: bool,
    instance: Option<String>,
}

impl HeaderDecoder {
    /// Construct a new [Header] decoder, which rejects instance and function names longer than
    /// `max_instance_len` and `max_name_len` bytes respectively
    #[must_use]
    pub fn new(max_instance_len: u32, max_name_len: u32) -> Self {
        Self {
            instance_dec: CoreNameDecoder::with_max_len(max_instance_len),
            name_dec: CoreNameDecoder::with_max_len(max_name_len),
            version: false,
            instance: None,
        }
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
            let Some(instance) = self.instance_dec.decode(src)? else {
                return Ok(None);
            };
            instance
        };
        let Some(name) = self.name_dec.decode(src)? else {
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

/// [Frame] decoder
pub struct Decoder {
    path: Option<Vec<usize>>,
    path_cap: usize,
    data_len: usize,
    max_depth: u32,
    max_size: u32,
}

impl Decoder {
    /// Construct a new [Frame] decoder
    #[must_use]
    pub fn new(max_depth: u32, max_size: u32) -> Self {
        Self {
            path: Option::default(),
            path_cap: 0,
            data_len: 0,
            max_depth,
            max_size,
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
        let path = self.path.take();
        let mut path = if let Some(path) = path {
            path
        } else {
            trace!("decoding path length");
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
            self.path_cap = n;
            Vec::with_capacity(n.min(MAX_INITIAL_PATH_CAPACITY))
        };
        while self.path_cap > 0 {
            trace!(self.path_cap, "decoding path element");
            let Some(i) = Leb128DecoderU32.decode(src)? else {
                self.path = Some(path);
                return Ok(None);
            };
            trace!(i, "decoded path element");
            let i = usize::try_from(i)
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
            path.push(i);
            self.path_cap -= 1;
        }
        if self.data_len == 0 {
            trace!("decoding data length");
            let Some(n) = Leb128DecoderU32.decode(src)? else {
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
            self.data_len = n;
            if n == 0 {
                return Ok(Some(Frame {
                    path: Arc::from(path),
                    data: Bytes::default(),
                }));
            }
        }
        let n = self.data_len.saturating_sub(src.len());
        if n > 0 {
            src.reserve(n.min(DEFAULT_MAX_INITIAL_CAPACITY));
            self.path = Some(path);
            return Ok(None);
        }
        trace!(self.data_len, "decoding data");
        let data = src.split_to(self.data_len).freeze();
        self.data_len = 0;
        Ok(Some(Frame {
            path: Arc::from(path),
            data,
        }))
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
    use tokio_util::codec::{Decoder as _, FramedRead, FramedWrite};

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
        crate::frame::encode_invocation(&mut buf, "foo", "bar").unwrap();
        let mut dec = HeaderDecoder::default();
        let mut src = BytesMut::default();
        let mut header = None;
        for b in &buf {
            src.extend_from_slice(&[*b]);
            header = dec.decode(&mut src).unwrap();
        }
        assert_eq!(
            header,
            Some(Header {
                instance: "foo".into(),
                name: "bar".into(),
            })
        );
        assert!(src.is_empty());
        let Err(HeaderReadError::IO(err)) =
            HeaderDecoder::new(2, 3).decode(&mut BytesMut::from(&buf[..2]))
        else {
            panic!("decoding a header with a too long instance name should fail");
        };
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

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
    }
}
