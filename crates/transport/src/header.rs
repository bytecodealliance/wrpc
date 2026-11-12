//! wRPC invocation header

use core::fmt::{Debug, Display};

use bytes::{Buf as _, BufMut as _, BytesMut};
use tokio_util::codec::Decoder as _;
use tracing::{instrument, trace};
use wasm_tokio::{CoreNameEncoder, Leb128DecoderU32};

use crate::DEFAULT_MAX_NAME_LEN;

/// wRPC protocol version
pub const PROTOCOL: u8 = 1;

/// Owned wRPC invocation header
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Header {
    /// Instance name of the function being called
    pub instance: String,

    /// Name of the function being called
    pub name: String,
}

/// wRPC invocation header reference
#[derive(Clone, Debug, Eq, PartialEq)]
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

/// Error returned by [`HeaderDecoder`]
pub enum HeaderReadError {
    /// I/O error
    IO(std::io::Error),
    /// Protocol version is not supported
    UnsupportedVersion(u8),
}

impl Debug for HeaderReadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::IO(err) => Debug::fmt(err, f),
            Self::UnsupportedVersion(v) => write!(f, "unsupported version byte: {v}"),
        }
    }
}

impl Display for HeaderReadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::IO(err) => Display::fmt(err, f),
            Self::UnsupportedVersion(v) => write!(f, "unsupported version byte: {v}"),
        }
    }
}

impl core::error::Error for HeaderReadError {}

impl From<std::io::Error> for HeaderReadError {
    fn from(err: std::io::Error) -> Self {
        Self::IO(err)
    }
}

/// [Header] decoder
#[derive(Debug)]
pub struct HeaderDecoder {
    max_instance_len: u32,
    max_name_len: u32,
    version: bool,
    instance: Option<String>,
    /// Length of the name being decoded, once decoded
    len: Option<usize>,
    /// Minimum number of bytes, which complete the header
    remaining: usize,
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
            remaining: 3,
        }
    }

    /// Returns the minimum number of bytes, which complete the header once [`decode`] returned
    /// [None]
    ///
    /// [`decode`]: tokio_util::codec::Decoder::decode
    #[must_use]
    pub fn min_remaining(&self) -> usize {
        self.remaining
    }

    /// Decodes a name of at most `max` bytes, which is followed by `rest` names
    fn decode_name(
        &mut self,
        src: &mut BytesMut,
        max: u32,
        rest: usize,
    ) -> std::io::Result<Option<String>> {
        let n = if let Some(n) = self.len.take() {
            n
        } else {
            let Some(n) = Leb128DecoderU32.decode(src)? else {
                self.remaining = rest.saturating_add(1);
                return Ok(None);
            };
            if n > max {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("name length of `{n}` exceeds maximum of `{max}`"),
                ));
            }
            usize::try_from(n)
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?
        };
        if src.len() < n {
            self.len = Some(n);
            self.remaining = n.saturating_sub(src.len()).saturating_add(rest);
            return Ok(None);
        }
        let name = src.split_to(n);
        let name = String::from_utf8(name.to_vec())
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
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
            let Some(instance) = self.decode_name(src, self.max_instance_len, 1)? else {
                return Ok(None);
            };
            instance
        };
        let Some(name) = self.decode_name(src, self.max_name_len, 0)? else {
            self.instance = Some(instance);
            return Ok(None);
        };
        self.version = false;
        self.remaining = 3;
        trace!(instance, name, "decoded header");
        Ok(Some(Header { instance, name }))
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if let Some(header) = self.decode(src)? {
            return Ok(Some(header));
        }
        if self.version || !src.is_empty() {
            return Err(HeaderReadError::IO(
                std::io::ErrorKind::UnexpectedEof.into(),
            ));
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
        let Err(HeaderReadError::IO(err)) =
            HeaderDecoder::new(2, 3).decode(&mut BytesMut::from(&buf[..2]))
        else {
            panic!("decoding a header with a too long instance name should fail");
        };
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
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
