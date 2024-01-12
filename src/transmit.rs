use core::fmt::{self, Display};

use async_trait::async_trait;
use bytes::Bytes;

#[derive(Debug)]
pub enum Error {
    Http(h2::Error),
    Io(std::io::Error),
}

impl Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(err) => write!(f, "http error: {err}"),
            Self::Io(err) => write!(f, "I/O error: {err}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result = core::result::Result<(), Error>;

#[async_trait]
pub trait Transmit {
    async fn transmit(self, s: h2::SendStream<Bytes>) -> Result;
}

#[async_trait]
impl Transmit for Bytes {
    async fn transmit(self, mut s: h2::SendStream<Bytes>) -> Result {
        s.reserve_capacity(self.len());
        s.send_data(self, true).map_err(Error::Http)
    }
}

#[async_trait]
impl Transmit for &'static [u8] {
    async fn transmit(self, s: h2::SendStream<Bytes>) -> Result {
        Bytes::transmit(Bytes::from_static(self), s).await
    }
}

#[async_trait]
impl Transmit for Vec<u8> {
    async fn transmit(self, s: h2::SendStream<Bytes>) -> Result {
        Bytes::transmit(Bytes::from(self), s).await
    }
}

#[async_trait]
impl Transmit for &'static str {
    async fn transmit(self, s: h2::SendStream<Bytes>) -> Result {
        self.as_bytes().transmit(s).await
    }
}

#[async_trait]
impl Transmit for String {
    async fn transmit(self, s: h2::SendStream<Bytes>) -> Result {
        self.into_bytes().transmit(s).await
    }
}

#[async_trait]
impl Transmit for u8 {
    async fn transmit(self, mut s: h2::SendStream<Bytes>) -> Result {
        // TODO: Trim zero
        s.reserve_capacity(1);
        s.send_data(vec![self].into(), true).map_err(Error::Http)
    }
}

#[async_trait]
impl Transmit for u16 {
    async fn transmit(self, mut s: h2::SendStream<Bytes>) -> Result {
        // TODO: Trim zeroes
        s.reserve_capacity(2);
        s.send_data(Bytes::from(self.to_le_bytes().to_vec()), true)
            .map_err(Error::Http)
    }
}

#[async_trait]
impl Transmit for u32 {
    async fn transmit(self, mut s: h2::SendStream<Bytes>) -> Result {
        // TODO: Trim zeroes
        s.reserve_capacity(4);
        s.send_data(Bytes::from(self.to_le_bytes().to_vec()), true)
            .map_err(Error::Http)
    }
}

#[async_trait]
impl Transmit for u64 {
    async fn transmit(self, mut s: h2::SendStream<Bytes>) -> Result {
        // TODO: Trim zeroes
        s.reserve_capacity(8);
        s.send_data(Bytes::from(self.to_le_bytes().to_vec()), true)
            .map_err(Error::Http)
    }
}

#[async_trait]
impl Transmit for u128 {
    async fn transmit(self, mut s: h2::SendStream<Bytes>) -> Result {
        // TODO: Trim zeroes
        s.reserve_capacity(16);
        s.send_data(Bytes::from(self.to_le_bytes().to_vec()), true)
            .map_err(Error::Http)
    }
}

#[async_trait]
impl Transmit for i8 {
    async fn transmit(self, mut s: h2::SendStream<Bytes>) -> Result {
        // TODO: Trim zero
        s.reserve_capacity(1);
        s.send_data(vec![self as u8].into(), true)
            .map_err(Error::Http)
    }
}

#[async_trait]
impl Transmit for i16 {
    async fn transmit(self, mut s: h2::SendStream<Bytes>) -> Result {
        // TODO: Trim zeroes
        s.reserve_capacity(2);
        s.send_data(Bytes::from(self.to_le_bytes().to_vec()), true)
            .map_err(Error::Http)
    }
}

#[async_trait]
impl Transmit for i32 {
    async fn transmit(self, mut s: h2::SendStream<Bytes>) -> Result {
        // TODO: Trim zeroes
        s.reserve_capacity(4);
        s.send_data(Bytes::from(self.to_le_bytes().to_vec()), true)
            .map_err(Error::Http)
    }
}

#[async_trait]
impl Transmit for i64 {
    async fn transmit(self, mut s: h2::SendStream<Bytes>) -> Result {
        // TODO: Trim zeroes
        s.reserve_capacity(8);
        s.send_data(Bytes::from(self.to_le_bytes().to_vec()), true)
            .map_err(Error::Http)
    }
}

#[async_trait]
impl Transmit for i128 {
    async fn transmit(self, mut s: h2::SendStream<Bytes>) -> Result {
        // TODO: Trim zeroes
        s.reserve_capacity(16);
        s.send_data(Bytes::from(self.to_le_bytes().to_vec()), true)
            .map_err(Error::Http)
    }
}
