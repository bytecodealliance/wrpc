//! wRPC invocation header acceptance

use bytes::BytesMut;
use tokio::io::{AsyncRead, AsyncReadExt as _};
use tokio_util::codec::Decoder as _;
use tracing::{instrument, trace};

use crate::{DEFAULT_MAX_NAME_LEN, Header, HeaderDecoder, HeaderReadError, READ_CHUNK_SIZE};

/// Reads the header of the invocation sent over `rx` into `buf`, see [`accept_with`]
///
/// # Errors
///
/// Returns an error if reading the header fails
pub async fn accept(
    rx: &mut (impl AsyncRead + Unpin),
    buf: &mut BytesMut,
) -> Result<Header, HeaderReadError> {
    accept_with(rx, buf, DEFAULT_MAX_NAME_LEN, DEFAULT_MAX_NAME_LEN).await
}

/// Reads the header of the invocation sent over `rx` into `buf`, rejecting instance and
/// function names longer than `max_instance_len` and `max_name_len` bytes respectively. The
/// bytes read past the header are left in `buf`, which must be passed to
/// [`serve`](crate::serve()) or [`serve_values`](crate::serve_values).
///
/// # Errors
///
/// Returns an error if reading the header fails
#[instrument(level = "trace", skip_all)]
pub async fn accept_with(
    rx: &mut (impl AsyncRead + Unpin),
    buf: &mut BytesMut,
    max_instance_len: u32,
    max_name_len: u32,
) -> Result<Header, HeaderReadError> {
    let mut dec = HeaderDecoder::new(max_instance_len, max_name_len);
    loop {
        if let Some(header) = dec.decode(buf)? {
            trace!(?header, "read invocation header");
            return Ok(header);
        }
        buf.reserve(READ_CHUNK_SIZE);
        let n = rx.read_buf(buf).await?;
        if n == 0 {
            return Err(HeaderReadError::IO(
                std::io::ErrorKind::UnexpectedEof.into(),
            ));
        }
    }
}
