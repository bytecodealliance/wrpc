//! wRPC invocation header acceptance

use bytes::{BufMut as _, BytesMut};
use tokio::io::{AsyncRead, AsyncReadExt as _};
use tokio_util::codec::Decoder as _;
use tracing::{instrument, trace};

use crate::{DEFAULT_MAX_NAME_LEN, Header, HeaderDecoder, HeaderReadError};

/// Reads the header of the invocation sent over `rx`, see [`accept_with`]
///
/// # Errors
///
/// Returns an error if reading the header fails
pub async fn accept(rx: &mut (impl AsyncRead + Unpin)) -> Result<Header, HeaderReadError> {
    accept_with(rx, DEFAULT_MAX_NAME_LEN, DEFAULT_MAX_NAME_LEN).await
}

/// Reads the header of the invocation sent over `rx`, rejecting instance and function names
/// longer than `max_instance_len` and `max_name_len` bytes respectively. Nothing past the header
/// is read.
///
/// # Errors
///
/// Returns an error if reading the header fails
#[instrument(level = "trace", skip_all)]
pub async fn accept_with(
    rx: &mut (impl AsyncRead + Unpin),
    max_instance_len: u32,
    max_name_len: u32,
) -> Result<Header, HeaderReadError> {
    let mut dec = HeaderDecoder::new(max_instance_len, max_name_len);
    let mut buf = BytesMut::default();
    loop {
        if let Some(header) = dec.decode(&mut buf)? {
            trace!(?header, "read invocation header");
            return Ok(header);
        }
        let n = dec.min_remaining();
        buf.reserve(n);
        let n = rx.read_buf(&mut (&mut buf).limit(n)).await?;
        if n == 0 {
            return Err(HeaderReadError::IO(
                std::io::ErrorKind::UnexpectedEof.into(),
            ));
        }
    }
}
