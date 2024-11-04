use core::marker::PhantomData;

use anyhow::Context as _;
use bytes::{BufMut as _, Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt as _};
use tokio_util::codec::Encoder;
use tracing::{instrument, trace};
use wasm_tokio::{CoreNameEncoder, CoreVecEncoderBytes};

use crate::frame::conn::{Incoming, Outgoing};
use crate::frame::{Conn, ConnHandler, PROTOCOL};

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
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Outgoing, Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
        I: AsyncRead + Unpin + Send + 'static,
        O: AsyncWrite + Unpin + Send + 'static,
        H: ConnHandler<I, O>,
    {
        let mut buf = BytesMut::with_capacity(
            17_usize // len(PROTOCOL) + len(instance) + len(func) + len([]) + len(params)
                .saturating_add(instance.len())
                .saturating_add(func.len())
                .saturating_add(params.len()),
        );
        buf.put_u8(PROTOCOL);
        CoreNameEncoder.encode(instance, &mut buf)?;
        CoreNameEncoder.encode(func, &mut buf)?;
        buf.put_u8(0);
        CoreVecEncoderBytes.encode(params, &mut buf)?;
        trace!(?buf, "writing invocation");
        tx.write_all(&buf)
            .await
            .context("failed to initialize connection")?;

        let Conn { tx, rx } = Conn::new::<H, _, _, _>(rx, tx, paths.as_ref());
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
    params: Bytes,
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
