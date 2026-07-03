use core::marker::PhantomData;

use anyhow::Context as _;
use bytes::{BufMut as _, BytesMut};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt as _};
use tokio_util::codec::Encoder;
use tracing::{instrument, trace};
use wasm_tokio::{CoreNameEncoder, CoreVecEncoderBytes};

use crate::frame::conn::{Incoming, Outgoing};
use crate::frame::{ConnHandler, PROTOCOL};

/// Encodes an invocation to a `BytesMut`
///
/// This is low-level API, most users should use [`invoke`].
#[instrument(level = "trace", skip_all)]
pub fn encode_invocation(
    buf: &mut BytesMut,
    instance: &str,
    func: &str,
    params: &[u8],
) -> std::io::Result<()> {
    buf.reserve(
        17_usize // len(PROTOCOL) + len(instance) + len(func) + len([]) + len(params)
            .saturating_add(instance.len())
            .saturating_add(func.len())
            .saturating_add(params.len()),
    );
    buf.put_u8(PROTOCOL);
    CoreNameEncoder.encode(instance, buf)?;
    CoreNameEncoder.encode(func, buf)?;
    buf.put_u8(0);
    CoreVecEncoderBytes.encode(params, buf)?;
    Ok(())
}

/// Defines invocation behavior
#[derive(Clone)]
pub struct InvokeBuilder<H = ()>(PhantomData<H>)
where
    H: ?Sized;

impl<H> InvokeBuilder<H> {
    /// Invoke function `func` on instance `instance`
    #[instrument(level = "trace", skip_all)]
    pub async fn invoke<P, I, O>(
        self,
        mut tx: O,
        rx: I,
        instance: &str,
        func: &str,
        params: impl AsRef<[u8]>,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Outgoing, Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
        I: AsyncRead + Unpin + Send + 'static,
        O: AsyncWrite + Unpin + Send + 'static,
        H: ConnHandler<I, O>,
    {
        let mut buf = BytesMut::default();
        encode_invocation(&mut buf, instance, func, params.as_ref())
            .context("failed to encode invocation")?;
        trace!(?buf, "writing invocation");
        tx.write_all(&buf)
            .await
            .context("failed to initialize connection")?;
        tx.flush().await.context("failed to flush invocation")?;

        let tx = Outgoing::new(tx, |tx, res| H::on_egress(tx, res));
        let rx = Incoming::new(rx, paths.as_ref(), |rx, res| H::on_ingress(rx, res));
        Ok((tx, rx))
    }
}

impl<H> Default for InvokeBuilder<H> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

/// Invoke function `func` on instance `instance`
#[instrument(level = "trace", skip_all)]
pub async fn invoke<P, I, O>(
    tx: O,
    rx: I,
    instance: &str,
    func: &str,
    params: impl AsRef<[u8]>,
    paths: impl AsRef<[P]> + Send,
) -> anyhow::Result<(Outgoing, Incoming)>
where
    P: AsRef<[Option<usize>]> + Send + Sync,
    I: AsyncRead + Unpin + Send + 'static,
    O: AsyncWrite + Unpin + Send + 'static,
{
    InvokeBuilder::<()>::default()
        .invoke(tx, rx, instance, func, params, paths)
        .await
}
