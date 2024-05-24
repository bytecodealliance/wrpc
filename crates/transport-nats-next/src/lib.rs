#![allow(clippy::type_complexity)]

use core::borrow::Borrow;
use core::convert::Infallible;
use core::future::Future;
use core::iter::zip;
use core::pin::{pin, Pin};
use core::task::{Context, Poll};
use core::{mem, str};

use std::sync::Arc;

use anyhow::{anyhow, ensure, Context as _};
use async_nats::client::Publisher;
use async_nats::{HeaderMap, Message, ServerInfo, StatusCode, Subject, Subscriber};
use bytes::{Buf as _, Bytes, BytesMut};
use futures::sink::SinkExt as _;
use futures::{Stream, StreamExt};
use tokio::io::{AsyncWrite, AsyncWriteExt as _};
use tokio::sync::oneshot;
use tokio::try_join;
use tokio_util::codec::Encoder;
use tokio_util::io::StreamReader;
use tracing::{instrument, trace, warn};
use wasm_tokio::{AsyncReadCore as _, CoreStringEncoder};
use wrpc_transport_next::Index as _;

pub const PROTOCOL: &str = "wrpc.0.0.1";

#[must_use]
#[inline]
pub fn error_subject(prefix: &str) -> String {
    format!("{prefix}.error")
}

#[must_use]
#[inline]
pub fn param_subject(prefix: &str) -> String {
    format!("{prefix}.params")
}

#[must_use]
#[inline]
pub fn result_subject(prefix: &str) -> String {
    format!("{prefix}.results")
}

#[must_use]
#[inline]
pub fn index_path(prefix: &str, path: &[usize]) -> String {
    let mut s = String::with_capacity(prefix.len() + path.len() * 2 - 1);
    for p in path {
        if !s.is_empty() {
            s.push('.');
        }
        s.push_str(&p.to_string());
    }
    s
}

#[must_use]
#[inline]
pub fn subsribe_path(prefix: &str, path: &[Option<usize>]) -> String {
    let mut s = String::with_capacity(prefix.len() + path.len() * 2 - 1);
    for p in path {
        if !s.is_empty() {
            s.push('.');
        }
        if let Some(p) = p {
            s.push_str(&p.to_string());
        } else {
            s.push('*');
        }
    }
    s
}

#[must_use]
#[inline]
pub fn invocation_subject(prefix: &str, instance: &str, func: &str) -> String {
    let mut s =
        String::with_capacity(prefix.len() + PROTOCOL.len() + instance.len() + func.len() + 3);
    if !prefix.is_empty() {
        s.push_str(prefix);
        s.push('.');
    }
    s.push_str(PROTOCOL);
    s.push('.');
    if !instance.is_empty() {
        s.push_str(instance);
        s.push('.');
    }
    s.push_str(func);
    s
}

fn corrupted_memory_error() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, "corrupted memory state")
}

#[derive(Clone, Debug)]
pub struct Client {
    nats: Arc<async_nats::Client>,
    prefix: Arc<str>,
}

impl Client {
    pub fn new(nats: impl Into<Arc<async_nats::Client>>, prefix: impl Into<Arc<str>>) -> Self {
        Self {
            nats: nats.into(),
            prefix: prefix.into(),
        }
    }
}

#[derive(Debug)]
pub struct ByteSubscription(Subscriber);

impl Stream for ByteSubscription {
    type Item = std::io::Result<Bytes>;

    #[instrument(level = "trace", skip_all)]
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.0.poll_next_unpin(cx) {
            Poll::Ready(Some(Message { payload, .. })) => Poll::Ready(Some(Ok(payload))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[derive(Default)]
enum SubscriberTree {
    #[default]
    Empty,
    Leaf(Subscriber),
    IndexNode {
        subscriber: Option<Subscriber>,
        nested: Vec<Option<SubscriberTree>>,
    },
    WildcardNode {
        subscriber: Option<Subscriber>,
        nested: Option<Box<SubscriberTree>>,
    },
}

impl<'a> From<(&'a [Option<usize>], Subscriber)> for SubscriberTree {
    fn from((path, sub): (&'a [Option<usize>], Subscriber)) -> Self {
        match path {
            [] => Self::Leaf(sub),
            [None, path @ ..] => Self::WildcardNode {
                subscriber: None,
                nested: Some(Box::new(Self::from((path, sub)))),
            },
            [Some(i), path @ ..] => Self::IndexNode {
                subscriber: None,
                nested: {
                    let n = i.saturating_add(1);
                    let mut nested = Vec::with_capacity(n);
                    nested.resize_with(n, Option::default);
                    nested[*i] = Some(Self::from((path, sub)));
                    nested
                },
            },
        }
    }
}

impl<'a, P: Borrow<&'a [Option<usize>]>> FromIterator<(P, Subscriber)> for SubscriberTree {
    fn from_iter<T: IntoIterator<Item = (P, Subscriber)>>(iter: T) -> Self {
        let mut root = Self::Empty;
        for (path, sub) in iter {
            if !root.insert(path.borrow(), sub) {
                return Self::Empty;
            }
        }
        root
    }
}

impl SubscriberTree {
    #[inline]
    fn is_empty(&self) -> bool {
        matches!(self, SubscriberTree::Empty)
    }

    #[instrument(level = "trace", skip_all)]
    fn take(&mut self, path: &[usize]) -> Option<Subscriber> {
        let Some((i, path)) = path.split_first() else {
            return match mem::take(self) {
                SubscriberTree::Empty => None,
                SubscriberTree::Leaf(subscriber) => Some(subscriber),
                SubscriberTree::IndexNode { subscriber, nested } => {
                    if !nested.is_empty() {
                        *self = SubscriberTree::IndexNode {
                            subscriber: None,
                            nested,
                        }
                    }
                    subscriber
                }
                SubscriberTree::WildcardNode { .. } => None,
                // TODO: Demux the subscription
                //SubscriberTree::WildcardNode { subscriber, nested } => {
                //    if let Some(nested) = nested {
                //        *self = SubscriberTree::WildcardNode {
                //            subscriber: None,
                //            nested: Some(nested),
                //        }
                //    }
                //    subscriber
                //}
            };
        };
        match self {
            Self::Empty | Self::Leaf(..) => None,
            Self::WildcardNode { .. } => None,
            // TODO: Demux the subscription
            //Self::WildcardNode { ref mut nested, .. } => {
            //    nested.as_mut().and_then(|nested| nested.take(path))
            //}
            Self::IndexNode { ref mut nested, .. } => nested
                .get_mut(*i)
                .and_then(|nested| nested.as_mut().and_then(|nested| nested.take(path))),
        }
    }

    /// Inserts `sub` under a `path` - returns `false` if it failed and `true` if it succeeded.
    /// Tree state after `false` is returned in undefined
    #[instrument(level = "trace", skip_all)]
    fn insert(&mut self, path: &[Option<usize>], sub: Subscriber) -> bool {
        match self {
            Self::Empty => {
                *self = Self::from((path, sub));
                true
            }
            Self::Leaf(..) => {
                let Some((i, path)) = path.split_first() else {
                    return false;
                };
                let Self::Leaf(subscriber) = mem::take(self) else {
                    return false;
                };
                if let Some(i) = i {
                    let n = i.saturating_add(1);
                    let mut nested = Vec::with_capacity(n);
                    nested.resize_with(n, Option::default);
                    nested[*i] = Some(Self::from((path, sub)));
                    *self = Self::IndexNode {
                        subscriber: Some(subscriber),
                        nested,
                    };
                } else {
                    *self = Self::WildcardNode {
                        subscriber: Some(subscriber),
                        nested: Some(Box::new(Self::from((path, sub)))),
                    };
                }
                true
            }
            Self::WildcardNode {
                ref mut subscriber,
                ref mut nested,
            } => match (&subscriber, path) {
                (None, []) => {
                    *subscriber = Some(sub);
                    true
                }
                (_, [None, path @ ..]) => {
                    if let Some(nested) = nested {
                        nested.insert(path, sub)
                    } else {
                        *nested = Some(Box::new(Self::from((path, sub))));
                        true
                    }
                }
                _ => false,
            },
            Self::IndexNode {
                ref mut subscriber,
                ref mut nested,
            } => match (&subscriber, path) {
                (None, []) => {
                    *subscriber = Some(sub);
                    true
                }
                (_, [Some(i), path @ ..]) => {
                    if nested.len() < *i {
                        nested.resize_with(i.saturating_add(1), Option::default);
                    }
                    let nested = &mut nested[*i];
                    if let Some(nested) = nested {
                        nested.insert(path, sub)
                    } else {
                        *nested = Some(Self::from((path, sub)));
                        true
                    }
                }
                _ => false,
            },
        }
    }
}

pub struct Reader {
    buffer: Bytes,
    incoming: Subscriber,
    nested: Arc<std::sync::Mutex<SubscriberTree>>,
}

impl wrpc_transport_next::Index<Reader> for Reader {
    type Error = anyhow::Error;

    #[instrument(level = "trace", skip_all)]
    fn index(&self, path: &[usize]) -> anyhow::Result<Reader> {
        let mut nested = self
            .nested
            .lock()
            .map_err(|err| anyhow!(err.to_string()).context("failed to lock map"))?;
        let incoming = nested.take(path).context("unknown subscription")?;
        Ok(Self {
            buffer: Bytes::default(),
            incoming,
            nested: Arc::clone(&self.nested),
        })
    }
}

impl tokio::io::AsyncRead for Reader {
    #[instrument(level = "trace", skip_all)]
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let cap = buf.remaining();
        if cap == 0 {
            return Poll::Ready(Ok(()));
        }

        if !self.buffer.is_empty() {
            if self.buffer.len() > cap {
                buf.put_slice(&self.buffer.split_to(cap));
            } else {
                buf.put_slice(&self.buffer);
            }
            return Poll::Ready(Ok(()));
        }
        match self.incoming.poll_next_unpin(cx) {
            Poll::Ready(Some(Message { mut payload, .. })) => {
                if payload.len() > cap {
                    buf.put_slice(&payload.split_to(cap));
                    self.buffer = payload;
                } else {
                    buf.put_slice(&payload);
                }
                Poll::Ready(Ok(()))
            }
            Poll::Ready(None) => Poll::Ready(Ok(())),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SubjectWriter {
    nats: Arc<async_nats::Client>,
    tx: Subject,
    publisher: Publisher,
}

impl SubjectWriter {
    fn new(nats: Arc<async_nats::Client>, tx: Subject, publisher: Publisher) -> Self {
        Self {
            nats,
            tx,
            publisher,
        }
    }
}

impl wrpc_transport_next::Index<SubjectWriter> for SubjectWriter {
    type Error = Infallible;

    #[instrument(level = "trace", skip_all)]
    fn index(&self, path: &[usize]) -> Result<SubjectWriter, Self::Error> {
        Ok(Self {
            nats: Arc::clone(&self.nats),
            tx: index_path(self.tx.as_str(), path).into(),
            publisher: self.publisher.clone(),
        })
    }
}

impl tokio::io::AsyncWrite for SubjectWriter {
    #[instrument(level = "trace", skip_all)]
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        trace!("polling for readiness");
        match self.publisher.poll_ready_unpin(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(..)) => return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into())),
            Poll::Ready(Ok(())) => {}
        }
        let ServerInfo { max_payload, .. } = self.nats.server_info();
        if buf.len() > max_payload {
            (buf, _) = buf.split_at(max_payload);
        }
        trace!("starting send");
        match self.publisher.start_send_unpin(Bytes::copy_from_slice(buf)) {
            Ok(()) => Poll::Ready(Ok(buf.len())),
            Err(..) => Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into())),
        }
    }

    #[instrument(level = "trace", skip_all)]
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        trace!("flushing");
        self.publisher
            .poll_flush_unpin(cx)
            .map_err(|_| std::io::ErrorKind::BrokenPipe.into())
    }

    #[instrument(level = "trace", skip_all)]
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        trace!("closing");
        self.publisher
            .poll_close_unpin(cx)
            .map_err(|_| std::io::ErrorKind::BrokenPipe.into())
    }
}

#[derive(Debug, Default)]
pub enum ParamWriter {
    #[default]
    Corrupted,
    Handshaking {
        tx: SubjectWriter,
        sub: Subscriber,
        indexed: std::sync::Mutex<Vec<(Vec<usize>, oneshot::Sender<SubjectWriter>)>>,
        error: oneshot::Sender<SubjectWriter>,
        buffer: Bytes,
    },
    Draining {
        tx: SubjectWriter,
        buffer: Bytes,
    },
    Active(SubjectWriter),
}

impl ParamWriter {
    fn new(
        tx: SubjectWriter,
        sub: Subscriber,
        error: oneshot::Sender<SubjectWriter>,
        buffer: Bytes,
    ) -> Self {
        Self::Handshaking {
            tx,
            sub,
            indexed: std::sync::Mutex::default(),
            error,
            buffer,
        }
    }
}

impl ParamWriter {
    #[instrument(level = "trace", skip_all)]
    fn poll_active(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut *self {
            Self::Corrupted => Poll::Ready(Err(corrupted_memory_error())),
            Self::Handshaking { sub, .. } => {
                trace!("polling for handshake response");
                match sub.poll_next_unpin(cx) {
                    Poll::Ready(Some(Message {
                        status: Some(StatusCode::NO_RESPONDERS),
                        ..
                    })) => Poll::Ready(Err(std::io::ErrorKind::NotConnected.into())),
                    Poll::Ready(Some(Message {
                        status: Some(StatusCode::TIMEOUT),
                        ..
                    })) => Poll::Ready(Err(std::io::ErrorKind::TimedOut.into())),
                    Poll::Ready(Some(Message {
                        status: Some(StatusCode::REQUEST_TERMINATED),
                        ..
                    })) => Poll::Ready(Err(std::io::ErrorKind::UnexpectedEof.into())),
                    Poll::Ready(Some(Message {
                        status: Some(code),
                        description,
                        ..
                    })) if !code.is_success() => Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        if let Some(description) = description {
                            format!("received a response with code `{code}` ({description})")
                        } else {
                            format!("received a response with code `{code}`")
                        },
                    ))),
                    Poll::Ready(Some(Message {
                        reply: Some(tx), ..
                    })) => {
                        let Self::Handshaking {
                            tx: SubjectWriter { nats, .. },
                            indexed,
                            error,
                            buffer,
                            ..
                        } = mem::take(&mut *self)
                        else {
                            return Poll::Ready(Err(corrupted_memory_error()));
                        };
                        let param_tx = Subject::from(param_subject(&tx));
                        let error_tx = Subject::from(error_subject(&tx));
                        error
                            .send(SubjectWriter::new(
                                Arc::clone(&nats),
                                error_tx.clone(),
                                nats.publish_sink(error_tx),
                            ))
                            .map_err(|_| std::io::Error::from(std::io::ErrorKind::BrokenPipe))?;
                        let param_pub = nats.publish_sink(param_tx.clone());
                        let tx = SubjectWriter::new(nats, param_tx, param_pub);
                        let indexed = indexed.into_inner().map_err(|err| {
                            std::io::Error::new(std::io::ErrorKind::Other, err.to_string())
                        })?;
                        for (path, tx_tx) in indexed {
                            let tx = tx.index(&path).map_err(|err| {
                                std::io::Error::new(std::io::ErrorKind::Other, err)
                            })?;
                            tx_tx.send(tx).map_err(|_| {
                                std::io::Error::from(std::io::ErrorKind::BrokenPipe)
                            })?;
                        }
                        trace!("handshake succeeded");
                        if buffer.is_empty() {
                            *self = Self::Active(tx);
                            Poll::Ready(Ok(()))
                        } else {
                            *self = Self::Draining { tx, buffer };
                            self.poll_active(cx)
                        }
                    }
                    Poll::Ready(Some(..)) => Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "peer did not specify a reply subject",
                    ))),
                    Poll::Ready(None) => {
                        *self = Self::Corrupted;
                        Poll::Ready(Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe)))
                    }
                    Poll::Pending => Poll::Pending,
                }
            }
            Self::Draining { tx, buffer } => {
                let mut tx = pin!(tx);
                while !buffer.is_empty() {
                    trace!(?tx.tx, "draining parameter buffer");
                    match tx.as_mut().poll_write(cx, buffer) {
                        Poll::Ready(Ok(n)) => {
                            buffer.advance(n);
                        }
                        Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                        Poll::Pending => return Poll::Pending,
                    }
                }
                let Self::Draining { tx, .. } = mem::take(&mut *self) else {
                    return Poll::Ready(Err(corrupted_memory_error()));
                };
                trace!("parameter buffer draining succeeded");
                *self = Self::Active(tx);
                Poll::Ready(Ok(()))
            }
            Self::Active(..) => Poll::Ready(Ok(())),
        }
    }
}

impl wrpc_transport_next::Index<IndexedParamWriter> for ParamWriter {
    type Error = std::io::Error;

    #[instrument(level = "trace", skip_all)]
    fn index(&self, path: &[usize]) -> std::io::Result<IndexedParamWriter> {
        match self {
            Self::Corrupted => Err(corrupted_memory_error()),
            Self::Handshaking { indexed, .. } => {
                let (tx_tx, tx_rx) = oneshot::channel();
                let mut indexed = indexed.lock().map_err(|err| {
                    std::io::Error::new(std::io::ErrorKind::Other, err.to_string())
                })?;
                indexed.push((path.to_vec(), tx_tx));
                Ok(IndexedParamWriter::Handshaking {
                    tx_rx,
                    indexed: std::sync::Mutex::default(),
                })
            }
            Self::Draining { tx, .. } | Self::Active(tx) => tx
                .index(path)
                .map(IndexedParamWriter::Active)
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err)),
        }
    }
}

impl tokio::io::AsyncWrite for ParamWriter {
    #[instrument(level = "trace", skip_all)]
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.as_mut().poll_active(cx)? {
            Poll::Ready(()) => {
                let Self::Active(tx) = &mut *self else {
                    return Poll::Ready(Err(corrupted_memory_error()));
                };
                trace!("writing buffer");
                pin!(tx).poll_write(cx, buf)
            }
            Poll::Pending => Poll::Pending,
        }
    }

    #[instrument(level = "trace", skip_all)]
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.as_mut().poll_active(cx)? {
            Poll::Ready(()) => {
                let Self::Active(tx) = &mut *self else {
                    return Poll::Ready(Err(corrupted_memory_error()));
                };
                trace!("flushing");
                pin!(tx).poll_flush(cx)
            }
            Poll::Pending => Poll::Pending,
        }
    }

    #[instrument(level = "trace", skip_all)]
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.as_mut().poll_active(cx)? {
            Poll::Ready(()) => {
                let Self::Active(tx) = &mut *self else {
                    return Poll::Ready(Err(corrupted_memory_error()));
                };
                trace!("shutting down");
                pin!(tx).poll_shutdown(cx)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[derive(Debug, Default)]
pub enum IndexedParamWriter {
    #[default]
    Corrupted,
    Handshaking {
        tx_rx: oneshot::Receiver<SubjectWriter>,
        indexed: std::sync::Mutex<Vec<(Vec<usize>, oneshot::Sender<SubjectWriter>)>>,
    },
    Active(SubjectWriter),
}

impl IndexedParamWriter {
    #[instrument(level = "trace", skip_all)]
    fn poll_active(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut *self {
            Self::Corrupted => Poll::Ready(Err(corrupted_memory_error())),
            Self::Handshaking { tx_rx, .. } => {
                trace!("polling for handshake");
                match pin!(tx_rx).poll(cx) {
                    Poll::Ready(Ok(tx)) => {
                        let Self::Handshaking { indexed, .. } = mem::take(&mut *self) else {
                            return Poll::Ready(Err(corrupted_memory_error()));
                        };
                        let indexed = indexed.into_inner().map_err(|err| {
                            std::io::Error::new(std::io::ErrorKind::Other, err.to_string())
                        })?;
                        for (path, tx_tx) in indexed {
                            let tx = tx.index(&path).map_err(|err| {
                                std::io::Error::new(std::io::ErrorKind::Other, err)
                            })?;
                            tx_tx.send(tx).map_err(|_| {
                                std::io::Error::from(std::io::ErrorKind::BrokenPipe)
                            })?;
                        }
                        *self = Self::Active(tx);
                        Poll::Ready(Ok(()))
                    }
                    Poll::Ready(Err(..)) => Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into())),
                    Poll::Pending => Poll::Pending,
                }
            }
            Self::Active(..) => Poll::Ready(Ok(())),
        }
    }
}

impl wrpc_transport_next::Index<Self> for IndexedParamWriter {
    type Error = std::io::Error;

    #[instrument(level = "trace", skip_all)]
    fn index(&self, path: &[usize]) -> std::io::Result<Self> {
        match self {
            Self::Corrupted => Err(corrupted_memory_error()),
            Self::Handshaking { indexed, .. } => {
                let (tx_tx, tx_rx) = oneshot::channel();
                let mut indexed = indexed.lock().map_err(|err| {
                    std::io::Error::new(std::io::ErrorKind::Other, err.to_string())
                })?;
                indexed.push((path.to_vec(), tx_tx));
                Ok(Self::Handshaking {
                    tx_rx,
                    indexed: std::sync::Mutex::default(),
                })
            }
            Self::Active(tx) => tx
                .index(path)
                .map(Self::Active)
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err)),
        }
    }
}

impl tokio::io::AsyncWrite for IndexedParamWriter {
    #[instrument(level = "trace", skip_all)]
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.as_mut().poll_active(cx)? {
            Poll::Ready(()) => {
                let Self::Active(tx) = &mut *self else {
                    return Poll::Ready(Err(corrupted_memory_error()));
                };
                trace!("writing buffer");
                pin!(tx).poll_write(cx, buf)
            }
            Poll::Pending => Poll::Pending,
        }
    }

    #[instrument(level = "trace", skip_all)]
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.as_mut().poll_active(cx)? {
            Poll::Ready(()) => {
                let Self::Active(tx) = &mut *self else {
                    return Poll::Ready(Err(corrupted_memory_error()));
                };
                trace!("flushing");
                pin!(tx).poll_flush(cx)
            }
            Poll::Pending => Poll::Pending,
        }
    }

    #[instrument(level = "trace", skip_all)]
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.as_mut().poll_active(cx)? {
            Poll::Ready(()) => {
                let Self::Active(tx) = &mut *self else {
                    return Poll::Ready(Err(corrupted_memory_error()));
                };
                trace!("shutting down");
                pin!(tx).poll_shutdown(cx)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[derive(Debug)]
pub enum ClientErrorWriter {
    Handshaking(oneshot::Receiver<SubjectWriter>),
    Active(SubjectWriter),
}

impl tokio::io::AsyncWrite for ClientErrorWriter {
    #[instrument(level = "trace", skip_all)]
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match &mut *self {
            Self::Handshaking(rx) => match pin!(rx).poll(cx) {
                Poll::Ready(Ok(tx)) => {
                    *self = Self::Active(tx);
                    self.poll_write(cx, buf)
                }
                Poll::Ready(Err(..)) => Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into())),
                Poll::Pending => Poll::Pending,
            },
            Self::Active(tx) => pin!(tx).poll_write(cx, buf),
        }
    }

    #[instrument(level = "trace", skip_all)]
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut *self {
            Self::Handshaking(rx) => match pin!(rx).poll(cx) {
                Poll::Ready(Ok(tx)) => {
                    *self = Self::Active(tx);
                    self.poll_flush(cx)
                }
                Poll::Ready(Err(..)) => Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into())),
                Poll::Pending => Poll::Pending,
            },
            Self::Active(tx) => pin!(tx).poll_flush(cx),
        }
    }

    #[instrument(level = "trace", skip_all)]
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut *self {
            Self::Handshaking(rx) => match pin!(rx).poll(cx) {
                Poll::Ready(Ok(tx)) => {
                    *self = Self::Active(tx);
                    self.poll_shutdown(cx)
                }
                Poll::Ready(Err(..)) => Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into())),
                Poll::Pending => Poll::Pending,
            },
            Self::Active(tx) => pin!(tx).poll_shutdown(cx),
        }
    }
}

#[derive(Debug)]
pub struct Session<O> {
    outgoing: O,
    incoming: Subscriber,
}

impl<O: AsyncWrite + Send> wrpc_transport_next::Session for Session<O> {
    type Error = String;
    type TransportError = anyhow::Error;

    #[instrument(level = "trace", skip_all)]
    async fn finish(
        mut self,
        res: Result<(), Self::Error>,
    ) -> Result<Result<(), Self::Error>, Self::TransportError> {
        if let Err(err) = res {
            let mut buf = BytesMut::with_capacity(5 + err.len());
            if let Err(err) = CoreStringEncoder.encode(err, &mut buf) {
                warn!(?err, "failed to encode error");
                buf.clear();
                if let Err(err) = CoreStringEncoder.encode(err.to_string(), &mut buf) {
                    warn!(?err, "failed to encode encoding error");
                    buf.clear();
                }
            }
            trace!(?buf, "writing error string");
            pin!(self.outgoing)
                .write_all(&buf)
                .await
                .context("failed to write error string")?;
        }
        trace!("unsubscribing from error subject");
        if let Err(err) = self.incoming.unsubscribe().await {
            warn!(?err, "failed to unsubscribe from error subject");
        }
        let incoming = ByteSubscription(self.incoming).peekable();
        let mut incoming = pin!(incoming);
        if (incoming.as_mut().peek().await).is_some() {
            let mut err = String::new();
            StreamReader::new(incoming)
                .read_core_string(&mut err)
                .await
                .context("failed to read error string")?;
            Ok(Err(err))
        } else {
            Ok(Ok(()))
        }
    }
}

impl wrpc_transport_next::Invoke for Client {
    type Error = anyhow::Error;
    type Context = Option<HeaderMap>;
    type Session = Session<ClientErrorWriter>;
    type Outgoing = ParamWriter;
    type NestedOutgoing = IndexedParamWriter;
    type Incoming = Reader;

    #[instrument(level = "trace", skip(self))]
    async fn invoke(
        &self,
        cx: Self::Context,
        instance: &str,
        func: &str,
        mut params: Bytes,
        paths: &[&[Option<usize>]],
    ) -> Result<
        wrpc_transport_next::Invocation<Self::Outgoing, Self::Incoming, Self::Session>,
        Self::Error,
    > {
        let rx = Subject::from(self.nats.new_inbox());
        let (result_rx, error_rx, handshake_rx, nested) = try_join!(
            async {
                self.nats
                    .subscribe(Subject::from(result_subject(&rx)))
                    .await
                    .context("failed to subscribe on result subject")
            },
            async {
                self.nats
                    .subscribe(Subject::from(error_subject(&rx)))
                    .await
                    .context("failed to subscribe on error subject")
            },
            async {
                self.nats
                    .subscribe(rx.clone())
                    .await
                    .context("failed to subscribe on handshake subject")
            },
            futures::future::try_join_all(paths.iter().map(|path| async {
                self.nats
                    .subscribe(Subject::from(subsribe_path(&rx, path)))
                    .await
                    .context("failed to subscribe on nested result subject")
            }))
        )?;
        let nested: SubscriberTree = zip(paths.iter(), nested).collect();
        ensure!(
            paths.is_empty() == nested.is_empty(),
            "failed to construct subscription tree"
        );
        let ServerInfo {
            mut max_payload, ..
        } = self.nats.server_info();
        max_payload = max_payload.saturating_sub(rx.len());
        let param_tx = Subject::from(invocation_subject(&self.prefix, instance, func));
        if let Some(headers) = cx {
            // based on https://github.com/nats-io/nats.rs/blob/0942c473ce56163fdd1fbc62762f8164e3afa7bf/async-nats/src/header.rs#L215-L224
            max_payload = max_payload
                .saturating_sub(b"NATS/1.0\r\n".len())
                .saturating_sub(b"\r\n".len());
            for (k, vs) in headers.iter() {
                let k: &[u8] = k.as_ref();
                for v in vs {
                    max_payload = max_payload
                        .saturating_sub(k.len())
                        .saturating_sub(b": ".len())
                        .saturating_sub(v.as_str().len())
                        .saturating_sub(b"\r\n".len());
                }
            }
            trace!("publishing handshake");
            self.nats
                .publish_with_reply_and_headers(
                    param_tx.clone(),
                    rx,
                    headers,
                    params.split_to(max_payload.min(params.len())),
                )
                .await
        } else {
            trace!("publishing handshake");
            self.nats
                .publish_with_reply(
                    param_tx.clone(),
                    rx,
                    params.split_to(max_payload.min(params.len())),
                )
                .await
        }
        .context("failed to send handshake")?;
        let (error_tx_tx, error_tx_rx) = oneshot::channel();
        Ok(wrpc_transport_next::Invocation {
            outgoing: ParamWriter::new(
                SubjectWriter::new(
                    Arc::clone(&self.nats),
                    param_tx.clone(),
                    self.nats.publish_sink(param_tx),
                ),
                handshake_rx,
                error_tx_tx,
                params,
            ),
            incoming: Reader {
                buffer: Bytes::default(),
                incoming: result_rx,
                nested: Arc::new(std::sync::Mutex::new(nested)),
            },
            session: Session {
                outgoing: ClientErrorWriter::Handshaking(error_tx_rx),
                incoming: error_rx,
            },
        })
    }
}

impl wrpc_transport_next::Serve for Client {
    type Error = anyhow::Error;
    type Context = Option<HeaderMap>;
    type Session = Session<SubjectWriter>;
    type Outgoing = SubjectWriter;
    type Incoming = Reader;

    #[instrument(level = "trace", skip(self))]
    async fn serve(
        &self,
        instance: &str,
        func: &str,
        paths: &[&[Option<usize>]],
    ) -> Result<
        impl Stream<
            Item = Result<
                (
                    Self::Context,
                    wrpc_transport_next::Invocation<Self::Outgoing, Self::Incoming, Self::Session>,
                ),
                Self::Error,
            >,
        >,
        Self::Error,
    > {
        let sub = self
            .nats
            .subscribe(invocation_subject(&self.prefix, instance, func))
            .await?;
        Ok(sub.then(
            |Message {
                 reply: tx,
                 payload,
                 headers,
                 ..
             }| async {
                let tx = tx.context("peer did not specify a reply subject")?;
                let rx = self.nats.new_inbox();
                let (param_rx, error_rx, nested) = try_join!(
                    async {
                        self.nats
                            .subscribe(Subject::from(param_subject(&rx)))
                            .await
                            .context("failed to subscribe on parameter subject")
                    },
                    async {
                        self.nats
                            .subscribe(Subject::from(error_subject(&rx)))
                            .await
                            .context("failed to subscribe on error subject")
                    },
                    futures::future::try_join_all(paths.iter().map(|path| async {
                        self.nats
                            .subscribe(Subject::from(subsribe_path(&rx, path)))
                            .await
                            .context("failed to subscribe on nested parameter subject")
                    }))
                )?;
                let nested: SubscriberTree = zip(paths.iter(), nested).collect();
                ensure!(
                    paths.is_empty() == nested.is_empty(),
                    "failed to construct subscription tree"
                );
                self.nats
                    .publish_with_reply(tx.clone(), rx, Bytes::default())
                    .await
                    .context("failed to publish handshake accept")?;
                let result_tx = Subject::from(result_subject(&tx));
                let error_tx = Subject::from(error_subject(&tx));
                Ok((
                    headers,
                    wrpc_transport_next::Invocation {
                        outgoing: SubjectWriter::new(
                            Arc::clone(&self.nats),
                            result_tx.clone(),
                            self.nats.publish_sink(result_tx),
                        ),
                        incoming: Reader {
                            buffer: payload,
                            incoming: param_rx,
                            nested: Arc::new(std::sync::Mutex::new(nested)),
                        },
                        session: Session {
                            outgoing: SubjectWriter::new(
                                Arc::clone(&self.nats),
                                error_tx.clone(),
                                self.nats.publish_sink(error_tx),
                            ),
                            incoming: error_rx,
                        },
                    },
                ))
            },
        ))
    }
}
