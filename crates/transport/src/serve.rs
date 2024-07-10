use core::future::Future;
use core::pin::Pin;

use std::sync::Arc;

use anyhow::{bail, Context as _};
use futures::{SinkExt as _, Stream, TryStreamExt as _};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt as _};
use tokio_util::codec::{FramedRead, FramedWrite};
use tracing::{debug, instrument, trace, Instrument as _, Span};

use crate::{Deferred as _, Index, TupleDecode, TupleEncode};

/// Server-side handle to a wRPC transport
pub trait Serve: Sync {
    /// Transport-specific invocation context
    type Context: Send + Sync + 'static;

    /// Outgoing multiplexed byte stream
    type Outgoing: AsyncWrite + Index<Self::Outgoing> + Send + Sync + Unpin + 'static;

    /// Incoming multiplexed byte stream
    type Incoming: AsyncRead + Index<Self::Incoming> + Send + Sync + Unpin + 'static;

    /// Serve function `func` from instance `instance`
    fn serve(
        &self,
        instance: &str,
        func: &str,
        paths: impl Into<Arc<[Box<[Option<usize>]>]>> + Send,
    ) -> impl Future<
        Output = anyhow::Result<
            impl Stream<Item = anyhow::Result<(Self::Context, Self::Outgoing, Self::Incoming)>>
                + Send
                + 'static,
        >,
    > + Send;
}

pub trait ServeExt: Serve {
    /// Serve function `func` from instance `instance` using typed `Params` and `Results`
    #[instrument(level = "trace", skip(self, paths))]
    fn serve_values<Params, Results>(
        &self,
        instance: &str,
        func: &str,
        paths: impl Into<Arc<[Box<[Option<usize>]>]>> + Send,
    ) -> impl Future<
        Output = anyhow::Result<
            impl Stream<
                    Item = anyhow::Result<(
                        Self::Context,
                        Params,
                        Option<impl Future<Output = std::io::Result<()>> + Send + Unpin + 'static>,
                        impl FnOnce(
                                Results,
                            )
                                -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>
                            + Send,
                    )>,
                > + Send
                + 'static,
        >,
    > + Send
    where
        Params: TupleDecode<Self::Incoming> + Send + 'static,
        Results: TupleEncode<Self::Outgoing> + Send + 'static,
        <Params::Decoder as tokio_util::codec::Decoder>::Error:
            std::error::Error + Send + Sync + 'static,
        <Results::Encoder as tokio_util::codec::Encoder<Results>>::Error:
            std::error::Error + Send + Sync + 'static,
    {
        async {
            let invocations = self.serve(instance, func, paths).await?;
            let span = Span::current();
            Ok(invocations.and_then(move |(cx, outgoing, incoming)| {
                async {
                    let mut dec = FramedRead::new(incoming, Params::Decoder::default());
                    debug!("receiving sync parameters");
                    let Some(params) = dec
                        .try_next()
                        .await
                        .context("failed to receive sync parameters")?
                    else {
                        bail!("incomplete sync parameters")
                    };
                    trace!("received sync parameters");
                    let rx = dec.decoder_mut().take_deferred();
                    let span = Span::current();
                    Ok((
                        cx,
                        params,
                        rx.map(|f| f(dec.into_inner().into(), Vec::with_capacity(8))),
                        move |results| {
                            Box::pin(
                                async {
                                    let mut enc =
                                        FramedWrite::new(outgoing, Results::Encoder::default());
                                    debug!("transmitting sync results");
                                    enc.send(results)
                                        .await
                                        .context("failed to transmit synchronous results")?;
                                    let tx = enc.encoder_mut().take_deferred();
                                    let mut outgoing = enc.into_inner();
                                    outgoing
                                        .shutdown()
                                        .await
                                        .context("failed to shutdown synchronous return channel")?;
                                    if let Some(tx) = tx {
                                        debug!("transmitting async results");
                                        tx(outgoing.into(), Vec::with_capacity(8))
                                            .await
                                            .context("failed to write async results")?;
                                    }
                                    Ok(())
                                }
                                .instrument(span),
                            ) as Pin<_>
                        },
                    ))
                }
                .instrument(span.clone())
            }))
        }
    }
}

impl<T: Serve> ServeExt for T {}
