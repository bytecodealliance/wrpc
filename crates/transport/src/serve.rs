//! wRPC transport server handle

use anyhow::Context as _;
use bytes::BytesMut;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::try_join;
use tokio_util::codec::Decoder;
use tracing::{Instrument as _, debug, trace};

use crate::frame::{Incoming, Outgoing};
use crate::mux::{egress, ingress};
use crate::{Deferred, Paths};

/// Serves an invocation, whose header was read by [`Header::read`](crate::Header::read), over
/// the byte streams `tx` and `rx`, decoding the parameters using `dec`
///
/// Returns a future receiving the parameters, which resolves to the synchronous parameters and
/// a future, which receives the asynchronous parameters and resolves once the invocation ends,
/// and the [Outgoing] handle the results are sent on using [`Outgoing::send`]. Nothing is
/// received unless it is polled.
pub fn serve<D>(
    tx: impl AsyncWrite + Send + Unpin + 'static,
    rx: impl AsyncRead + Send + Unpin + 'static,
    mut dec: D,
) -> (
    impl Future<
        Output = anyhow::Result<(
            D::Item,
            impl Future<Output = anyhow::Result<()>> + Send + 'static,
        )>,
    > + Send,
    Outgoing,
)
where
    D: Decoder + Deferred<Incoming> + Paths + Send,
    D::Item: Send,
    D::Error: core::error::Error + Send + Sync + 'static,
{
    let mut incoming = ingress(rx, dec.paths());
    (
        async move {
            debug!("receiving sync parameters");
            let params = incoming
                .decode(&mut dec)
                .await
                .context("failed to receive sync parameters")?;
            let params = params.context("incomplete sync parameters")?;
            trace!("received sync parameters");
            let rx = dec.take_deferred();
            Ok((
                params,
                async move {
                    let closed = incoming.closed();
                    let rx = rx.map(|rx| rx(&incoming, &[])).transpose();
                    let rx = rx.context("failed to index async parameters")?;
                    try_join!(
                        async {
                            if let Some(rx) = rx {
                                debug!("receiving async parameters");
                                rx.await.context("failed to receive async parameters")?;
                            }
                            Ok(())
                        },
                        async {
                            incoming
                                .read_end()
                                .await
                                .context("failed to receive sync parameters")
                        },
                    )?;
                    drop(incoming);
                    closed.await.context("failed to receive parameters")
                }
                .in_current_span(),
            ))
        }
        .in_current_span(),
        egress(tx, BytesMut::default()),
    )
}
