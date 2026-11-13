//! wRPC transport server handle

use core::pin::Pin;

use anyhow::Context as _;
use bytes::BytesMut;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::try_join;
use tokio_util::codec::BytesCodec;
use tracing::{Instrument as _, Span, debug, instrument, trace};

use crate::frame::{Incoming, Outgoing};
use crate::mux::mux;
use crate::{Deferred as _, Paths as _, TupleDecode, TupleEncode};

/// Multiplexes the byte streams `tx` and `rx` of an invocation, whose header was read by
/// [`accept`](crate::accept()), declaring the async `paths`, see [`invoke`](crate::invoke())
#[instrument(level = "trace", skip_all)]
pub fn serve(
    tx: impl AsyncWrite + Send + Unpin + 'static,
    rx: impl AsyncRead + Send + Unpin + 'static,
    paths: impl IntoIterator<Item = impl AsRef<[Option<usize>]>>,
) -> (Outgoing, Incoming) {
    mux(tx, rx, BytesMut::default(), paths)
}

/// Serves an invocation, whose header was read by [`accept`](crate::accept()), over the byte
/// streams `tx` and `rx` using typed `Params` and `Results`
///
/// Returns a future receiving the parameters, which resolves to the synchronous parameters and
/// a future, which receives the asynchronous parameters and resolves once the invocation ends,
/// and a function transmitting the results. Nothing is received unless it is polled.
pub fn serve_values<Params, Results>(
    tx: impl AsyncWrite + Send + Unpin + 'static,
    rx: impl AsyncRead + Send + Unpin + 'static,
) -> (
    impl Future<
        Output = anyhow::Result<(
            Params,
            impl Future<Output = anyhow::Result<()>> + Send + 'static,
        )>,
    > + Send
    + 'static,
    impl FnOnce(Results) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'static>>
    + Send
    + 'static,
)
where
    Params: TupleDecode + Send,
    Results: TupleEncode + Send,
    <Params::Decoder as tokio_util::codec::Decoder>::Error:
        core::error::Error + Send + Sync + 'static,
    <Results::Encoder as tokio_util::codec::Encoder<Results>>::Error:
        core::error::Error + Send + Sync + 'static,
{
    let span = Span::current();
    let (mut outgoing, mut incoming) = serve(tx, rx, Params::Decoder::default().paths());
    (
        async move {
            let mut dec = Params::Decoder::default();
            debug!("receiving sync parameters");
            let params = incoming
                .decode(&mut dec)
                .await
                .context("failed to receive sync parameters")?;
            let params = params.context("incomplete sync parameters")?;
            trace!("received sync parameters");
            Ok((
                params,
                async move {
                    let closed = incoming.closed();
                    let rx = dec.take_deferred().map(|rx| rx(&incoming, &[])).transpose();
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
        .instrument(span.clone()),
        move |results| {
            let mut buf = BytesMut::default();
            let tx = results.encode(&mut Results::Encoder::default(), &mut buf);
            Box::pin(
                async move {
                    let tx = tx.context("failed to encode results")?;
                    debug!("transmitting sync results");
                    outgoing
                        .encode(&mut BytesCodec::default(), buf)
                        .await
                        .context("failed to write sync results")?;
                    let tx = tx.map(|tx| tx(&outgoing, &[])).transpose();
                    let tx = tx.context("failed to index async results")?;
                    outgoing
                        .close()
                        .await
                        .context("failed to close result channel")?;
                    if let Some(tx) = tx {
                        debug!("transmitting async results");
                        tx.await.context("failed to write async results")?;
                    }
                    Ok(())
                }
                .instrument(span),
            )
        },
    )
}
