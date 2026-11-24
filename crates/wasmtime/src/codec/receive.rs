use core::future::pending;
use core::mem;
use core::task::{Context, Poll, ready};

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::BytesMut;
use futures::stream;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::PollSender;
use tracing::error;
use wasmtime::component::Type;
use wrpc_core::{Receive, ReceiveError};

use crate::{TupleDecoder, Val};

/// Receives the payload of a pending `future` on its path and the values nested in it below `0`
struct FutureReceiver {
    ty: Option<Type>,
    dec: TupleDecoder,
    /// Sender of the payload, until it is received
    tx: Option<oneshot::Sender<Option<Val>>>,
    buffer: BytesMut,
    closed: bool,
}

impl Receive for FutureReceiver {
    fn poll_receive(
        &mut self,
        cx: &mut Context<'_>,
        path: &[usize],
        src: &mut BytesMut,
    ) -> Poll<Result<(), ReceiveError>> {
        if let Some((i, rest)) = path.split_first() {
            if *i != 0 || self.tx.is_some() {
                return Poll::Ready(Err(ReceiveError::UnknownPath));
            }
            return self.dec.poll_receive(cx, rest, src);
        }
        let Some(tx) = self.tx.take() else {
            if !src.is_empty() || self.closed {
                return Poll::Ready(Err(ReceiveError::DataAfterEnd));
            }
            self.closed = true;
            return Poll::Ready(Ok(()));
        };
        if src.is_empty() {
            self.closed = true;
        } else {
            self.buffer.extend_from_slice(src);
            src.clear();
        }
        let v = if let Some(ty) = &self.ty {
            let v = self.dec.decode_val(&mut self.buffer, ty)?;
            v.map(Some)
        } else {
            Some(None)
        };
        let Some(v) = v else {
            if self.closed {
                return Poll::Ready(Err(ReceiveError::Truncated));
            }
            self.tx = Some(tx);
            return Poll::Ready(Ok(()));
        };
        if !self.buffer.is_empty() {
            return Poll::Ready(Err(ReceiveError::DataAfterEnd));
        }
        if tx.send(v).is_err() {
            return Poll::Ready(Err(ReceiveError::Unread));
        }
        Poll::Ready(Ok(()))
    }

    fn is_complete(&self) -> bool {
        self.closed && self.dec.is_complete()
    }
}

/// Returns a pending `future` with payload of type `ty` and its receiver
pub(super) fn pending_future(ty: Option<Type>, max_len: u32) -> (Val, Box<dyn Receive + Send>) {
    let (tx, rx) = oneshot::channel();
    let r = FutureReceiver {
        ty,
        dec: TupleDecoder::new(Arc::default()).with_max_len(max_len),
        tx: Some(tx),
        buffer: BytesMut::default(),
        closed: false,
    };
    let v = Box::pin(async {
        let Ok(v) = rx.await else {
            error!("future I/O dropped");
            return pending().await;
        };
        Ok(v)
    });
    (Val::Future(v), Box::new(r))
}

/// Receives the chunks of a pending `stream` on its path and the values nested in its elements
/// below their indices
struct StreamReceiver {
    ty: Type,
    dec: TupleDecoder,
    /// Sender of the chunks and of the end, until the path is closed
    tx: Option<(PollSender<Vec<Val>>, oneshot::Sender<()>)>,
    buffer: BytesMut,
    /// Length and decoded elements of a partially-received list
    list: Option<(usize, Vec<Val>)>,
    /// Index of the next element
    offset: usize,
    /// Decoders of the elements with pending asynchronous values
    nested: BTreeMap<usize, TupleDecoder>,
}

impl StreamReceiver {
    fn decode_chunk(&mut self) -> std::io::Result<Vec<Val>> {
        let max_len = self.dec.max_len;
        let mut chunk = Vec::default();
        loop {
            let (n, mut vs) = if let Some(list) = self.list.take() {
                list
            } else {
                let Some(n) = self.dec.decode_len(&mut self.buffer)? else {
                    return Ok(chunk);
                };
                (n, Vec::default())
            };
            while vs.len() < n {
                let Some(v) = self.dec.decode_val(&mut self.buffer, &self.ty)? else {
                    self.list = Some((n, vs));
                    return Ok(chunk);
                };
                if !self.dec.is_complete() {
                    let dec = TupleDecoder::new(Arc::default()).with_max_len(max_len);
                    let dec = mem::replace(&mut self.dec, dec);
                    self.nested
                        .insert(self.offset.saturating_add(vs.len()), dec);
                }
                vs.push(v);
            }
            self.offset = self.offset.checked_add(n).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "stream element index would overflow usize",
                )
            })?;
            if chunk.len().saturating_add(n) > usize::try_from(max_len).unwrap_or(usize::MAX) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("stream chunk exceeds the maximum of `{max_len}` elements"),
                ));
            }
            chunk.extend(vs);
        }
    }
}

impl Receive for StreamReceiver {
    fn poll_receive(
        &mut self,
        cx: &mut Context<'_>,
        path: &[usize],
        src: &mut BytesMut,
    ) -> Poll<Result<(), ReceiveError>> {
        if !path.is_empty() {
            return self.nested.poll_receive(cx, path, src);
        }
        if src.is_empty() {
            let Some((.., end)) = self.tx.take() else {
                return Poll::Ready(Err(ReceiveError::DataAfterEnd));
            };
            if self.list.is_some() || !self.buffer.is_empty() {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "stream closed within a chunk",
                )
                .into()));
            }
            _ = end.send(());
            return Poll::Ready(Ok(()));
        }
        let Some((tx, ..)) = &mut self.tx else {
            return Poll::Ready(Err(ReceiveError::DataAfterEnd));
        };
        ready!(tx.poll_reserve(cx)).map_err(|_| ReceiveError::Unread)?;
        self.buffer.extend_from_slice(src);
        src.clear();
        let chunk = self.decode_chunk()?;
        if !chunk.is_empty()
            && let Some((tx, ..)) = &mut self.tx
        {
            tx.send_item(chunk).map_err(|_| ReceiveError::Unread)?;
        }
        Poll::Ready(Ok(()))
    }

    fn is_complete(&self) -> bool {
        self.tx.is_none() && self.nested.is_complete()
    }
}

/// Returns a pending `stream` with elements of type `ty` and its receiver
pub(super) fn pending_stream(ty: Type, max_len: u32) -> (Val, Box<dyn Receive + Send>) {
    let (tx, rx) = mpsc::channel(1);
    let (end_tx, end_rx) = oneshot::channel();
    let r = StreamReceiver {
        ty,
        dec: TupleDecoder::new(Arc::default()).with_max_len(max_len),
        tx: Some((PollSender::new(tx), end_tx)),
        buffer: BytesMut::default(),
        list: None,
        offset: 0,
        nested: BTreeMap::default(),
    };
    let items = stream::unfold((rx, end_rx), |(mut rx, end)| async {
        if let Some(chunk) = rx.recv().await {
            return Some((chunk, (rx, end)));
        }
        if end.await.is_err() {
            error!("stream I/O dropped");
            return pending().await;
        }
        None
    });
    (Val::Stream(Box::pin(items)), Box::new(r))
}
