use std::sync::Arc;

use anyhow::Context as _;
use bytes::{BufMut as _, Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt as _};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::codec::Encoder;
use tokio_util::io::StreamReader;
use tokio_util::sync::PollSender;
use tracing::{debug, error, instrument, trace, Instrument as _};
use wasm_tokio::{CoreNameEncoder, CoreVecEncoderBytes};

use crate::frame::conn::{egress, ingress, Incoming, Outgoing};
use crate::frame::PROTOCOL;

/// Invoke function `func` on instance `instance`
#[instrument(level = "trace", skip_all)]
pub async fn invoke<P, I, O>(
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

    let index = Arc::new(std::sync::Mutex::new(paths.as_ref().iter().collect()));
    let (results_tx, results_rx) = mpsc::channel(1);
    let mut results_io = JoinSet::new();
    results_io.spawn({
        let index = Arc::clone(&index);
        async move {
            if let Err(err) = ingress(rx, index, results_tx).await {
                error!(?err, "result ingress failed")
            } else {
                debug!("result ingress successfully complete")
            }
        }
        .in_current_span()
    });

    let (params_tx, params_rx) = mpsc::channel(1);
    tokio::spawn(
        async {
            if let Err(err) = egress(params_rx, tx).await {
                error!(?err, "parameter egress failed")
            } else {
                debug!("parameter egress successfully complete")
            }
        }
        .in_current_span(),
    );
    Ok((
        Outgoing {
            tx: PollSender::new(params_tx),
            path: Arc::from([]),
            path_buf: Bytes::from_static(&[0]),
        },
        Incoming {
            rx: Some(StreamReader::new(ReceiverStream::new(results_rx))),
            path: Arc::from([]),
            index,
            io: Arc::new(results_io),
        },
    ))
}
