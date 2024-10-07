use core::mem;
use core::pin::Pin;
use core::task::{ready, Context, Poll};

use std::sync::Arc;

use anyhow::ensure;
use bytes::{Buf as _, BufMut as _, Bytes, BytesMut};
use futures::Sink as _;
use pin_project_lite::pin_project;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt as _};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::codec::Encoder;
use tokio_util::io::StreamReader;
use tokio_util::sync::PollSender;
use tracing::{instrument, trace};
use wasm_tokio::{AsyncReadLeb128 as _, Leb128Encoder};

use crate::Index;

mod accept;
mod client;
mod server;

pub use accept::*;
pub use client::*;
pub use server::*;

#[derive(Default)]
pub enum IndexTrie {
    #[default]
    Empty,
    Leaf {
        tx: mpsc::Sender<std::io::Result<Bytes>>,
        rx: Option<mpsc::Receiver<std::io::Result<Bytes>>>,
    },
    IndexNode {
        tx: Option<mpsc::Sender<std::io::Result<Bytes>>>,
        rx: Option<mpsc::Receiver<std::io::Result<Bytes>>>,
        nested: Vec<Option<IndexTrie>>,
    },
    // TODO: Add partially-indexed `WildcardIndexNode`
    WildcardNode {
        tx: Option<mpsc::Sender<std::io::Result<Bytes>>>,
        rx: Option<mpsc::Receiver<std::io::Result<Bytes>>>,
        nested: Option<Box<IndexTrie>>,
    },
}

impl<'a>
    From<(
        &'a [Option<usize>],
        mpsc::Sender<std::io::Result<Bytes>>,
        Option<mpsc::Receiver<std::io::Result<Bytes>>>,
    )> for IndexTrie
{
    fn from(
        (path, tx, rx): (
            &'a [Option<usize>],
            mpsc::Sender<std::io::Result<Bytes>>,
            Option<mpsc::Receiver<std::io::Result<Bytes>>>,
        ),
    ) -> Self {
        match path {
            [] => Self::Leaf { tx, rx },
            [None, path @ ..] => Self::WildcardNode {
                tx: None,
                rx: None,
                nested: Some(Box::new(Self::from((path, tx, rx)))),
            },
            [Some(i), path @ ..] => Self::IndexNode {
                tx: None,
                rx: None,
                nested: {
                    let n = i.saturating_add(1);
                    let mut nested = Vec::with_capacity(n);
                    nested.resize_with(n, Option::default);
                    nested[*i] = Some(Self::from((path, tx, rx)));
                    nested
                },
            },
        }
    }
}

impl<'a>
    From<(
        &'a [Option<usize>],
        mpsc::Sender<std::io::Result<Bytes>>,
        mpsc::Receiver<std::io::Result<Bytes>>,
    )> for IndexTrie
{
    fn from(
        (path, tx, rx): (
            &'a [Option<usize>],
            mpsc::Sender<std::io::Result<Bytes>>,
            mpsc::Receiver<std::io::Result<Bytes>>,
        ),
    ) -> Self {
        Self::from((path, tx, Some(rx)))
    }
}

impl<'a> From<(&'a [Option<usize>], mpsc::Sender<std::io::Result<Bytes>>)> for IndexTrie {
    fn from((path, tx): (&'a [Option<usize>], mpsc::Sender<std::io::Result<Bytes>>)) -> Self {
        Self::from((path, tx, None))
    }
}

impl<P: AsRef<[Option<usize>]>> FromIterator<P> for IndexTrie {
    fn from_iter<T: IntoIterator<Item = P>>(iter: T) -> Self {
        let mut root = Self::Empty;
        for path in iter {
            let (tx, rx) = mpsc::channel(16);
            if !root.insert(path.as_ref(), tx, Some(rx)) {
                return Self::Empty;
            }
        }
        root
    }
}

impl IndexTrie {
    #[instrument(level = "trace", skip(self), ret(level = "trace"))]
    fn take_rx(&mut self, path: &[usize]) -> Option<mpsc::Receiver<std::io::Result<Bytes>>> {
        let Some((i, path)) = path.split_first() else {
            return match self {
                Self::Empty => None,
                Self::Leaf { rx, .. } => rx.take(),
                Self::IndexNode { tx, rx, nested } => {
                    let rx = rx.take();
                    if nested.is_empty() && tx.is_none() {
                        *self = Self::Empty;
                    }
                    rx
                }
                Self::WildcardNode { tx, rx, nested } => {
                    let rx = rx.take();
                    if nested.is_none() && tx.is_none() {
                        *self = Self::Empty;
                    }
                    rx
                }
            };
        };
        match self {
            Self::Empty | Self::Leaf { .. } | Self::WildcardNode { .. } => None,
            Self::IndexNode { ref mut nested, .. } => nested
                .get_mut(*i)
                .and_then(|nested| nested.as_mut().and_then(|nested| nested.take_rx(path))),
            // TODO: Demux the subscription
            //Self::WildcardNode { ref mut nested, .. } => {
            //    nested.as_mut().and_then(|nested| nested.take(path))
            //}
        }
    }

    #[instrument(level = "trace", skip(self), ret(level = "trace"))]
    fn get_tx(&mut self, path: &[usize]) -> Option<mpsc::Sender<std::io::Result<Bytes>>> {
        let Some((i, path)) = path.split_first() else {
            return match self {
                Self::Empty => None,
                Self::Leaf { tx, .. } => Some(tx.clone()),
                Self::IndexNode { tx, .. } | Self::WildcardNode { tx, .. } => tx.clone(),
            };
        };
        match self {
            Self::Empty | Self::Leaf { .. } | Self::WildcardNode { .. } => None,
            Self::IndexNode { ref mut nested, .. } => {
                let nested = nested.get_mut(*i)?;
                let nested = nested.as_mut()?;
                nested.get_tx(path)
            } // TODO: Demux the subscription
              //Self::WildcardNode { ref mut nested, .. } => {
              //    nested.as_mut().and_then(|nested| nested.take(path))
              //}
        }
    }

    /// Inserts `sender` and `receiver` under a `path` - returns `false` if it failed and `true` if it succeeded.
    /// Tree state after `false` is returned is undefined
    #[instrument(level = "trace", skip(self, sender, receiver), ret(level = "trace"))]
    fn insert(
        &mut self,
        path: &[Option<usize>],
        sender: mpsc::Sender<std::io::Result<Bytes>>,
        receiver: Option<mpsc::Receiver<std::io::Result<Bytes>>>,
    ) -> bool {
        match self {
            Self::Empty => {
                *self = Self::from((path, sender, receiver));
                true
            }
            Self::Leaf { .. } => {
                let Some((i, path)) = path.split_first() else {
                    return false;
                };
                let Self::Leaf { tx, rx } = mem::take(self) else {
                    return false;
                };
                if let Some(i) = i {
                    let n = i.saturating_add(1);
                    let mut nested = Vec::with_capacity(n);
                    nested.resize_with(n, Option::default);
                    nested[*i] = Some(Self::from((path, sender, receiver)));
                    *self = Self::IndexNode {
                        tx: Some(tx),
                        rx,
                        nested,
                    };
                } else {
                    *self = Self::WildcardNode {
                        tx: Some(tx),
                        rx,
                        nested: Some(Box::new(Self::from((path, sender, receiver)))),
                    };
                }
                true
            }
            Self::IndexNode {
                ref mut tx,
                ref mut rx,
                ref mut nested,
            } => match (&tx, &rx, path) {
                (None, None, []) => {
                    *tx = Some(sender);
                    *rx = receiver;
                    true
                }
                (_, _, [Some(i), path @ ..]) => {
                    let cap = i.saturating_add(1);
                    if nested.len() < cap {
                        nested.resize_with(cap, Option::default);
                    }
                    let nested = &mut nested[*i];
                    if let Some(nested) = nested {
                        nested.insert(path, sender, receiver)
                    } else {
                        *nested = Some(Self::from((path, sender, receiver)));
                        true
                    }
                }
                _ => false,
            },
            Self::WildcardNode {
                ref mut tx,
                ref mut rx,
                ref mut nested,
            } => match (&tx, &rx, path) {
                (None, None, []) => {
                    *tx = Some(sender);
                    *rx = receiver;
                    true
                }
                (_, _, [None, path @ ..]) => {
                    if let Some(nested) = nested {
                        nested.insert(path, sender, receiver)
                    } else {
                        *nested = Some(Box::new(Self::from((path, sender, receiver))));
                        true
                    }
                }
                _ => false,
            },
        }
    }
}

pin_project! {
    #[project = IncomingProj]
    pub struct Incoming {
        #[pin]
        rx: Option<StreamReader<ReceiverStream<std::io::Result<Bytes>>, Bytes>>,
        path: Arc<[usize]>,
        index: Arc<std::sync::Mutex<IndexTrie>>,
        io: Arc<JoinSet<()>>,
    }
}

impl Index<Self> for Incoming {
    #[instrument(level = "trace", skip(self), fields(path = ?self.path))]
    fn index(&self, path: &[usize]) -> anyhow::Result<Self> {
        ensure!(!path.is_empty());
        let path = if self.path.is_empty() {
            Arc::from(path)
        } else {
            Arc::from([self.path.as_ref(), path].concat())
        };
        trace!("locking index trie");
        let mut index = self
            .index
            .lock()
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err.to_string()))?;
        trace!(?path, "taking index subscription");
        let rx = index
            .take_rx(&path)
            .map(|rx| StreamReader::new(ReceiverStream::new(rx)));
        Ok(Self {
            rx,
            path,
            index: Arc::clone(&self.index),
            io: Arc::clone(&self.io),
        })
    }
}

impl AsyncRead for Incoming {
    #[instrument(level = "trace", skip_all, fields(path = ?self.path), ret(level = "trace"))]
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        trace!("reading");
        let this = self.as_mut().project();
        let Some(rx) = this.rx.as_pin_mut() else {
            trace!("reader is closed");
            return Poll::Ready(Ok(()));
        };
        ready!(rx.poll_read(cx, buf))?;
        trace!(?buf, "read buffer");
        if buf.filled().is_empty() {
            self.rx.take();
        }
        Poll::Ready(Ok(()))
    }
}

pin_project! {
    #[project = OutgoingProj]
    pub struct Outgoing {
        #[pin]
        tx: PollSender<(Bytes, Bytes)>,
        path: Arc<[usize]>,
        path_buf: Bytes,
    }
}

impl Index<Self> for Outgoing {
    #[instrument(level = "trace", skip(self), fields(path = ?self.path))]
    fn index(&self, path: &[usize]) -> anyhow::Result<Self> {
        ensure!(!path.is_empty());
        let path: Arc<[usize]> = if self.path.is_empty() {
            Arc::from(path)
        } else {
            Arc::from([self.path.as_ref(), path].concat())
        };
        let mut buf = BytesMut::with_capacity(path.len().saturating_add(5));
        let n = u32::try_from(path.len())
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
        trace!(n, "encoding path length");
        Leb128Encoder.encode(n, &mut buf)?;
        for p in path.as_ref() {
            let p = u32::try_from(*p)
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
            trace!(p, "encoding path element");
            Leb128Encoder.encode(p, &mut buf)?;
        }
        Ok(Self {
            tx: self.tx.clone(),
            path,
            path_buf: buf.freeze(),
        })
    }
}

impl AsyncWrite for Outgoing {
    #[instrument(level = "trace", skip_all, fields(path = ?self.path, buf = format!("{buf:02x?}")), ret(level = "trace"))]
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        trace!("writing outgoing chunk");
        let mut this = self.project();
        ready!(this.tx.as_mut().poll_ready(cx))
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::BrokenPipe, err))?;
        this.tx
            .start_send((this.path_buf.clone(), Bytes::copy_from_slice(buf)))
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::BrokenPipe, err))?;
        Poll::Ready(Ok(buf.len()))
    }

    #[instrument(level = "trace", skip_all, fields(path = ?self.path), ret(level = "trace"))]
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    #[instrument(level = "trace", skip_all, fields(path = ?self.path), ret(level = "trace"))]
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[instrument(level = "trace", skip_all, ret(level = "trace"))]
async fn ingress(
    mut rx: impl AsyncRead + Unpin,
    index: Arc<std::sync::Mutex<IndexTrie>>,
    param_tx: mpsc::Sender<std::io::Result<Bytes>>,
) -> std::io::Result<()> {
    loop {
        trace!("reading path length");
        let b = match rx.read_u8().await {
            Ok(b) => b,
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(err) => return Err(err),
        };
        let n = AsyncReadExt::chain([b].as_slice(), &mut rx)
            .read_u32_leb128()
            .await?;
        let n = n
            .try_into()
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
        trace!(n, "read path length");
        let tx = if n == 0 {
            &param_tx
        } else {
            let mut path = Vec::with_capacity(n);
            for i in 0..n {
                trace!(i, "reading path element");
                let p = rx.read_u32_leb128().await?;
                let p = usize::try_from(p)
                    .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
                path.push(p);
            }
            trace!(?path, "read path");

            trace!("locking index trie");
            let mut index = index
                .lock()
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err.to_string()))?;
            &index.get_tx(&path).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("`{path:?}` subscription not found"),
                )
            })?
        };
        trace!("reading data length");
        let n = rx.read_u32_leb128().await?;
        let n = n
            .try_into()
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
        trace!(n, "read data length");
        let mut buf = BytesMut::with_capacity(n);
        buf.put_bytes(0, n);
        rx.read_exact(&mut buf).await?;
        tx.send(Ok(buf.freeze())).await.map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "stream receiver closed")
        })?;
    }
}

#[instrument(level = "trace", skip_all, ret(level = "trace"))]
async fn egress(
    mut rx: mpsc::Receiver<(Bytes, Bytes)>,
    mut tx: impl AsyncWrite + Unpin,
) -> std::io::Result<()> {
    let mut buf = BytesMut::with_capacity(5);
    trace!("waiting for next frame");
    while let Some((path, data)) = rx.recv().await {
        let data_len = u32::try_from(data.len())
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
        buf.clear();
        Leb128Encoder.encode(data_len, &mut buf)?;
        let mut frame = path.chain(&mut buf).chain(data);
        trace!(?frame, "writing egress frame");
        tx.write_all_buf(&mut frame).await?;
    }
    Ok(())
}
