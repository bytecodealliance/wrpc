use core::fmt::{Debug, Display};

use std::collections::{hash_map, HashMap};
use std::sync::Arc;

use anyhow::bail;
use bytes::Bytes;
use futures::{Stream, StreamExt as _};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinSet;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::io::StreamReader;
use tokio_util::sync::PollSender;
use tracing::{debug, error, instrument, trace, Instrument as _, Span};
use wasm_tokio::AsyncReadCore as _;

use crate::frame::conn::{egress, ingress, Accept};
use crate::frame::{Incoming, Outgoing};
use crate::Serve;

pub struct Server<C, I, O>(Mutex<HashMap<String, HashMap<String, mpsc::Sender<(C, I, O)>>>>);

impl<C, I, O> Default for Server<C, I, O> {
    fn default() -> Self {
        Self(Mutex::default())
    }
}

pub enum AcceptError<C, I, O> {
    IO(std::io::Error),
    UnsupportedVersion(u8),
    UnhandledFunction { instance: String, name: String },
    Send(mpsc::error::SendError<(C, I, O)>),
}

impl<C, I, O> Debug for AcceptError<C, I, O> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AcceptError::IO(err) => Debug::fmt(err, f),
            AcceptError::UnsupportedVersion(v) => write!(f, "unsupported version byte: {v}"),
            AcceptError::UnhandledFunction { instance, name } => {
                write!(f, "`{instance}#{name}` does not have a handler registered")
            }
            AcceptError::Send(err) => Debug::fmt(err, f),
        }
    }
}

impl<C, I, O> Display for AcceptError<C, I, O> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AcceptError::IO(err) => Display::fmt(err, f),
            AcceptError::UnsupportedVersion(v) => write!(f, "unsupported version byte: {v}"),
            AcceptError::UnhandledFunction { instance, name } => {
                write!(f, "`{instance}#{name}` does not have a handler registered")
            }
            AcceptError::Send(err) => Display::fmt(err, f),
        }
    }
}

impl<C, I, O> std::error::Error for AcceptError<C, I, O> {}

impl<C, I, O> Server<C, I, O>
where
    I: AsyncRead + Unpin,
{
    /// Accept a connection on an [Accept].
    ///
    /// # Errors
    ///
    /// Returns an error if accepting the connection has failed
    #[instrument(level = "trace", skip_all, ret(level = "trace"))]
    pub async fn accept(
        &self,
        listener: impl Accept<Context = C, Incoming = I, Outgoing = O>,
    ) -> Result<(), AcceptError<C, I, O>> {
        let (cx, tx, mut rx) = listener.accept().await.map_err(AcceptError::IO)?;
        let mut instance = String::default();
        let mut name = String::default();
        match rx.read_u8().await.map_err(AcceptError::IO)? {
            0x00 => {
                rx.read_core_name(&mut instance)
                    .await
                    .map_err(AcceptError::IO)?;
                rx.read_core_name(&mut name)
                    .await
                    .map_err(AcceptError::IO)?;
            }
            v => return Err(AcceptError::UnsupportedVersion(v)),
        }
        let h = self.0.lock().await;
        let h = h
            .get(&instance)
            .and_then(|h| h.get(&name))
            .ok_or_else(|| AcceptError::UnhandledFunction { instance, name })?;
        h.send((cx, rx, tx)).await.map_err(AcceptError::Send)?;
        Ok(())
    }
}

#[instrument(level = "trace", skip(srv, paths))]
async fn serve<C, I, O>(
    srv: &Server<C, I, O>,
    instance: &str,
    func: &str,
    paths: impl Into<Arc<[Box<[Option<usize>]>]>> + Send,
) -> anyhow::Result<impl Stream<Item = anyhow::Result<(C, Outgoing, Incoming)>> + 'static>
where
    C: Send + Sync + 'static,
    I: AsyncRead + Send + Sync + Unpin + 'static,
    O: AsyncWrite + Send + Sync + Unpin + 'static,
{
    let (tx, rx) = mpsc::channel(1024);
    let mut handlers = srv.0.lock().await;
    match handlers
        .entry(instance.to_string())
        .or_default()
        .entry(func.to_string())
    {
        hash_map::Entry::Occupied(_) => {
            bail!("handler for `{instance}#{func}` already exists")
        }
        hash_map::Entry::Vacant(entry) => {
            entry.insert(tx);
        }
    }
    let paths = paths.into();
    let span = Span::current();
    Ok(ReceiverStream::new(rx).then(move |(cx, rx, tx)| {
        let _span = span.enter();
        trace!("received invocation");
        let index = Arc::new(std::sync::Mutex::new(paths.as_ref().iter().collect()));
        async move {
            let (params_tx, params_rx) = mpsc::channel(1);
            let mut params_io = JoinSet::new();
            params_io.spawn({
                let index = Arc::clone(&index);
                async move {
                    if let Err(err) = ingress(rx, index, params_tx).await {
                        error!(?err, "parameter ingress failed")
                    } else {
                        debug!("parameter ingress successfully complete")
                    }
                }
                .in_current_span()
            });

            let (results_tx, results_rx) = mpsc::channel(1);
            tokio::spawn(
                async {
                    if let Err(err) = egress(results_rx, tx).await {
                        error!(?err, "result egress failed")
                    } else {
                        debug!("result egress successfully complete")
                    }
                }
                .in_current_span(),
            );
            Ok((
                cx,
                Outgoing {
                    tx: PollSender::new(results_tx),
                    path: Arc::from([]),
                    path_buf: Bytes::from_static(&[0]),
                },
                Incoming {
                    rx: Some(StreamReader::new(ReceiverStream::new(params_rx))),
                    path: Arc::from([]),
                    index,
                    io: Arc::new(params_io),
                },
            ))
        }
        .instrument(span.clone())
    }))
}

impl<C, I, O> Serve for Server<C, I, O>
where
    C: Send + Sync + 'static,
    I: AsyncRead + Send + Sync + Unpin + 'static,
    O: AsyncWrite + Send + Sync + Unpin + 'static,
{
    type Context = C;
    type Outgoing = Outgoing;
    type Incoming = Incoming;

    async fn serve(
        &self,
        instance: &str,
        func: &str,
        paths: impl Into<Arc<[Box<[Option<usize>]>]>> + Send,
    ) -> anyhow::Result<
        impl Stream<Item = anyhow::Result<(Self::Context, Self::Outgoing, Self::Incoming)>> + 'static,
    > {
        serve(self, instance, func, paths).await
    }
}

impl<C, I, O> Serve for &Server<C, I, O>
where
    C: Send + Sync + 'static,
    I: AsyncRead + Send + Sync + Unpin + 'static,
    O: AsyncWrite + Send + Sync + Unpin + 'static,
{
    type Context = C;
    type Outgoing = Outgoing;
    type Incoming = Incoming;

    async fn serve(
        &self,
        instance: &str,
        func: &str,
        paths: impl Into<Arc<[Box<[Option<usize>]>]>> + Send,
    ) -> anyhow::Result<
        impl Stream<Item = anyhow::Result<(Self::Context, Self::Outgoing, Self::Incoming)>> + 'static,
    > {
        serve(self, instance, func, paths).await
    }
}
