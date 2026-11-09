use anyhow::Context as _;
use bytes::{BufMut as _, BytesMut};
use tokio::io::{AsyncBufRead, AsyncWrite, AsyncWriteExt as _};
use tokio_util::codec::Encoder;
use tracing::{instrument, trace};
use wasm_tokio::CoreNameEncoder;

use crate::frame::PROTOCOL;
use crate::frame::conn::{Incoming, Outgoing};

/// Encodes the header of an invocation to a `BytesMut`.
///
/// This is low-level API, most users should use [`invoke`].
#[instrument(level = "trace", skip_all)]
pub fn encode_invocation(buf: &mut BytesMut, instance: &str, func: &str) -> std::io::Result<()> {
    buf.reserve(
        11_usize // len(PROTOCOL) + len(instance) + len(func)
            .saturating_add(instance.len())
            .saturating_add(func.len()),
    );
    buf.put_u8(PROTOCOL);
    CoreNameEncoder.encode(instance, buf)?;
    CoreNameEncoder.encode(func, buf)?;
    Ok(())
}

/// Invoke function `func` on instance `instance`, writing the invocation header without
/// flushing it
#[instrument(level = "trace", skip_all)]
pub async fn invoke<P, I, O>(
    mut tx: O,
    rx: I,
    instance: &str,
    func: &str,
    paths: impl AsRef<[P]> + Send,
) -> anyhow::Result<(Outgoing, Incoming)>
where
    P: AsRef<[Option<usize>]> + Send + Sync,
    I: AsyncBufRead + Unpin + Send + 'static,
    O: AsyncWrite + Unpin + Send + 'static,
{
    let mut buf = BytesMut::default();
    encode_invocation(&mut buf, instance, func).context("failed to encode invocation")?;
    trace!(?buf, "writing invocation");
    tx.write_all_buf(&mut buf)
        .await
        .context("failed to initialize connection")?;

    let tx = Outgoing::new(tx);
    let rx = Incoming::new(rx, paths.as_ref());
    Ok((tx, rx))
}
