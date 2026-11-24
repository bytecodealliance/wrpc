//! wRPC invocation header

use core::error::Error;
use core::fmt;
use core::str::Utf8Error;

use bytes::{Buf as _, BufMut as _, BytesMut};
use tokio_util::codec::Decoder as _;
use tracing::{instrument, trace};
use wasm_tokio::{CoreNameEncoder, Leb128DecoderU32};

/// wRPC protocol version
pub const PROTOCOL: u8 = 1;

/// Default maximum length of the instance and function names in a [Header]
const DEFAULT_MAX_NAME_LEN: u32 = 1 << 10;

/// Owned wRPC invocation header
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Header {
    /// Instance name of the function being called
    pub instance: String,

    /// Name of the function being called
    pub name: String,
}

/// wRPC invocation header reference
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeaderRef<'a> {
    /// Instance name of the function being called
    pub instance: &'a str,
    /// Name of the function being called
    pub name: &'a str,
}

impl<'a> From<&'a Header> for HeaderRef<'a> {
    fn from(Header { instance, name }: &'a Header) -> Self {
        Self { instance, name }
    }
}

/// Error of decoding a [Header]
#[derive(Debug)]
pub enum HeaderDecodeError {
    /// The protocol version byte is not [PROTOCOL]
    UnsupportedVersion(u8),
    /// An instance or function name is longer than the limit of the decoder
    NameTooLong {
        /// Length of the name
        len: u32,
        /// Maximum length of the name
        max: u32,
    },
    /// An instance or function name is not valid UTF-8
    InvalidName(Utf8Error),
    /// A name length does not fit in a `u32`
    Overflow,
    /// The stream ended inside a header
    Truncated,
    /// Reading the stream failed
    Io(std::io::Error),
}

impl fmt::Display for HeaderDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion(v) => write!(f, "unsupported version byte `{v}`"),
            Self::NameTooLong { len, max } => {
                write!(f, "name length of `{len}` exceeds maximum of `{max}`")
            }
            Self::InvalidName(..) => f.write_str("name is not valid UTF-8"),
            Self::Overflow => f.write_str("name length does not fit in `u32`"),
            Self::Truncated => f.write_str("stream ended inside a header"),
            Self::Io(..) => f.write_str("failed to read header"),
        }
    }
}

impl Error for HeaderDecodeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidName(err) => Some(err),
            Self::Io(err) => Some(err),
            Self::UnsupportedVersion(..)
            | Self::NameTooLong { .. }
            | Self::Overflow
            | Self::Truncated => None,
        }
    }
}

impl From<std::io::Error> for HeaderDecodeError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<HeaderDecodeError> for std::io::Error {
    fn from(err: HeaderDecodeError) -> Self {
        match err {
            HeaderDecodeError::Io(err) => err,
            HeaderDecodeError::Truncated => Self::new(std::io::ErrorKind::UnexpectedEof, err),
            err => Self::new(std::io::ErrorKind::InvalidData, err),
        }
    }
}

/// [Header] decoder, which can be used, for example, with [`FramedRead`](tokio_util::codec::FramedRead)
/// and switched to the [`FrameDecoder`](crate::FrameDecoder) of the invocation following the
/// header using [`FramedRead::map_decoder`](tokio_util::codec::FramedRead::map_decoder)
#[derive(Clone, Debug)]
pub struct HeaderDecoder {
    max_instance_len: u32,
    max_name_len: u32,
    version: bool,
    instance: Option<String>,
    /// Length of the name being decoded, once decoded
    len: Option<usize>,
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
            instance: None,
            len: None,
        }
    }

    /// Decodes a name of at most `max` bytes
    fn decode_name(
        &mut self,
        src: &mut BytesMut,
        max: u32,
    ) -> Result<Option<String>, HeaderDecodeError> {
        let n = if let Some(n) = self.len.take() {
            n
        } else {
            let Some(n) = Leb128DecoderU32
                .decode(src)
                .map_err(|_| HeaderDecodeError::Overflow)?
            else {
                return Ok(None);
            };
            if n > max {
                return Err(HeaderDecodeError::NameTooLong { len: n, max });
            }
            usize::try_from(n).map_err(|_| HeaderDecodeError::Overflow)?
        };
        if src.len() < n {
            self.len = Some(n);
            return Ok(None);
        }
        let name = src.split_to(n);
        let name = String::from_utf8(name.to_vec())
            .map_err(|err| HeaderDecodeError::InvalidName(err.utf8_error()))?;
        Ok(Some(name))
    }
}

impl Default for HeaderDecoder {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_NAME_LEN, DEFAULT_MAX_NAME_LEN)
    }
}

impl tokio_util::codec::Decoder for HeaderDecoder {
    type Item = Header;
    type Error = HeaderDecodeError;

    #[instrument(level = "trace", skip_all)]
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if !self.version {
            let Some(v) = src.first().copied() else {
                return Ok(None);
            };
            if v != PROTOCOL {
                return Err(HeaderDecodeError::UnsupportedVersion(v));
            }
            src.advance(1);
            self.version = true;
        }
        let instance = if let Some(instance) = self.instance.take() {
            instance
        } else {
            let Some(instance) = self.decode_name(src, self.max_instance_len)? else {
                return Ok(None);
            };
            instance
        };
        let Some(name) = self.decode_name(src, self.max_name_len)? else {
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
        if self.version || !src.is_empty() {
            return Err(HeaderDecodeError::Truncated);
        }
        Ok(None)
    }
}

/// [Header] encoder
#[derive(Clone, Copy, Debug, Default)]
pub struct HeaderEncoder;

impl tokio_util::codec::Encoder<HeaderRef<'_>> for HeaderEncoder {
    type Error = std::io::Error;

    #[instrument(level = "trace", skip_all)]
    fn encode(
        &mut self,
        HeaderRef { instance, name }: HeaderRef<'_>,
        dst: &mut BytesMut,
    ) -> Result<(), Self::Error> {
        dst.reserve(
            11_usize // len(PROTOCOL) + len(instance) + len(name)
                .saturating_add(instance.len())
                .saturating_add(name.len()),
        );
        dst.put_u8(PROTOCOL);
        CoreNameEncoder.encode(instance, dst)?;
        CoreNameEncoder.encode(name, dst)
    }
}

#[cfg(test)]
mod tests {
    use tokio_util::codec::Encoder as _;

    use super::*;

    #[test]
    fn header_limits() {
        let mut buf = BytesMut::default();
        HeaderEncoder
            .encode(
                HeaderRef {
                    instance: "foo",
                    name: "bar",
                },
                &mut buf,
            )
            .unwrap();
        let err = HeaderDecoder::new(2, 3)
            .decode(&mut BytesMut::from(&buf[..2]))
            .expect_err("decoding a header with a too long instance name should fail");
        assert!(
            matches!(err, HeaderDecodeError::NameTooLong { len: 3, max: 2 }),
            "{err:?}"
        );
        let mut dec = HeaderDecoder::default();
        let mut src = BytesMut::default();
        let mut header = None;
        for b in buf {
            src.put_u8(b);
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
    }
}
