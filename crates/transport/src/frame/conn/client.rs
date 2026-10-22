use core::marker::PhantomData;

use anyhow::Context as _;
use bytes::{Buf as _, BufMut as _, BytesMut};
use tokio::io::{AsyncBufRead, AsyncWrite, AsyncWriteExt as _};
use tokio_util::codec::Encoder;
use tracing::{instrument, trace};
use wasm_tokio::{CoreNameEncoder, Leb128Encoder};

use crate::frame::conn::{Incoming, Outgoing};
use crate::frame::{ConnHandler, PROTOCOL};

/// Encodes the header of an invocation, which is followed by `params_len` bytes of
/// parameters, to a `BytesMut`
///
/// This is low-level API, most users should use [`invoke`].
#[instrument(level = "trace", skip_all)]
pub fn encode_invocation(
    buf: &mut BytesMut,
    instance: &str,
    func: &str,
    params_len: usize,
) -> std::io::Result<()> {
    let params_len = u32::try_from(params_len)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
    buf.reserve(
        17_usize // len(PROTOCOL) + len(instance) + len(func) + len([]) + len(params)
            .saturating_add(instance.len())
            .saturating_add(func.len()),
    );
    buf.put_u8(PROTOCOL);
    CoreNameEncoder.encode(instance, buf)?;
    CoreNameEncoder.encode(func, buf)?;
    buf.put_u8(0);
    Leb128Encoder.encode(params_len, buf)?;
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
        I: AsyncBufRead + Unpin + Send + 'static,
        O: AsyncWrite + Unpin + Send + 'static,
        H: ConnHandler<I, O>,
    {
        let params = params.as_ref();
        let mut buf = BytesMut::default();
        encode_invocation(&mut buf, instance, func, params.len())
            .context("failed to encode invocation")?;
        trace!(?buf, "writing invocation");
        tx.write_all_buf(&mut buf.chain(params))
            .await
            .context("failed to initialize connection")?;
        tx.flush().await.context("failed to flush invocation")?;

        let tx = Outgoing::new(tx, |tx, res| H::on_egress(tx, res));
        let rx = Incoming::new(rx, paths.as_ref(), H::DRAIN, |rx, res| {
            H::on_ingress(rx, res)
        });
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
    I: AsyncBufRead + Unpin + Send + 'static,
    O: AsyncWrite + Unpin + Send + 'static,
{
    InvokeBuilder::<()>::default()
        .invoke(tx, rx, instance, func, params, paths)
        .await
}
