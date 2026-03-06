//! wRPC Zenoh transport

#![allow(clippy::type_complexity)]

use anyhow::{anyhow, ensure, Context as _};
use wrpc_transport::Index;
use zenoh::bytes::ZBytes;
use zenoh::sample::Sample;

use core::ops::{Deref, DerefMut};
use std::iter::zip;
use std::{io, mem};
use zenoh::{Session, Wait};

use core::future::Future;
use core::pin::{pin, Pin};
use core::task::{Context, Poll};
use core::{str};

use std::collections::{HashMap};
use std::sync::{Arc};

use bytes::{Buf as _, Bytes};
use futures::{Stream, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::select;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;
use tokio_stream::wrappers::ReceiverStream;

use tracing::{debug, error, instrument, trace, warn};

pub const PROTOCOL: &str = "wrpc.0.0.1";


fn send_sync(session: &Session, key: &str, payload: &[u8]) -> io::Result<()> {
    // Using the synchronous (blocking) API due to some synchronization issue that nats handles 
    session
        .put(key, payload)
        .wait()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    Ok(())
}


fn spawn_async(fut: impl Future<Output = ()> + Send + 'static) {
    match tokio::runtime::Handle::try_current() {
        Ok(rt) => {
            rt.spawn(fut);
        }
        Err(_) => match tokio::runtime::Runtime::new() {
            Ok(rt) => {
                rt.spawn(fut);
            }
            Err(err) => error!(?err, "failed to create a new Tokio runtime"),
        },
    }
}

fn zbytes_as_bytes(zbytes: &ZBytes) -> Bytes {
    // Get &ZBytes from the sample
    let bytes = match zbytes.to_bytes() {
                std::borrow::Cow::Borrowed(slice) => Bytes::copy_from_slice(slice),
                std::borrow::Cow::Owned(vec) => Bytes::from(vec),
    };

    bytes
}

fn child_inbox(base: &str) -> String {
    let base = if base.ends_with('/') {
        base
    } else {
        &format!("{base}/")
    };
    format!("{base}{}", nuid::next())
}

#[must_use]
#[inline]
pub fn param_subject(prefix: &str) -> String {
    format!("{prefix}/params")
}

#[must_use]
#[inline]
pub fn result_subject(prefix: &str) -> String {
    format!("{prefix}/results")
}

#[must_use]
#[inline]
pub fn index_path(prefix: &str, path: &[usize]) -> String {
    let mut s = String::with_capacity(prefix.len() + path.len() * 2);
    if !prefix.is_empty() {
        s.push_str(prefix);
    }
    for p in path {
        if !s.is_empty() {
            s.push('/');
        }
        s.push_str(&p.to_string());
    }
    s
}

#[must_use]
#[inline]
pub fn subscribe_path(prefix: &str, path: &[Option<usize>]) -> String {
    let mut s = String::with_capacity(prefix.len() + path.len() * 2);
    if !prefix.is_empty() {
        s.push_str(prefix);
    }
    for p in path {
        if !s.is_empty() {
            s.push('/');
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
        s.push('/');
    }
    s.push_str(PROTOCOL);
    s.push('/');
    if !instance.is_empty() {
        s.push_str(instance);
        s.push('/');
    }
    s.push_str(func);
    s
}

fn corrupted_memory_error() -> std::io::Error {
    std::io::Error::other("corrupted memory state")
}

/// Transport subscriber
pub struct Subscriber {
    rx: ReceiverStream<Sample>,
    subject: String,
    commands: mpsc::Sender<Command>,
    tasks: Arc<JoinSet<()>>,
}

#[hotpath::measure_all]
impl Drop for Subscriber {
    fn drop(&mut self) {
        let commands = self.commands.clone();
        let subject = self.subject.clone();
        let tasks = Arc::clone(&self.tasks);
        spawn_async(async move {
            trace!(?subject, "shutting down subscriber");
            if let Err(err) = commands.send(Command::Unsubscribe(subject)).await {
                warn!(?err, "failed to shutdown subscriber");
            }
            drop(tasks);
        });
    }
}

#[hotpath::measure_all]
impl Deref for Subscriber {
    type Target = ReceiverStream<Sample>;

    fn deref(&self) -> &Self::Target {
        &self.rx
    }
}

#[hotpath::measure_all]
impl DerefMut for Subscriber {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.rx
    }
}

enum Command {
    Subscribe(String, mpsc::Sender<Sample>),
    Unsubscribe(String),
    Batch(Box<[Command]>, oneshot::Sender<()>),
}

#[derive(Clone, Debug)]
pub struct Client {
    session: Arc<zenoh::Session>,
    prefix: Arc<str>,
    inbox: Arc<str>,
    //queue_group: Option<Arc<str>>,
    commands: mpsc::Sender<Command>,
    tasks: Arc<JoinSet<()>>,
}

#[hotpath::measure_all]
impl Client {
    pub async fn new(
        session: impl Into<Arc<zenoh::Session>>,
        prefix: impl Into<Arc<str>>,
        //queue_group: Option<Arc<str>>,
    ) -> anyhow::Result<Self> {
        let session: Arc<zenoh::Session> = session.into();
        let root = format!("_inbox/{}/", nuid::next());
        let wildcard = format!("{root}**");
        let sub = session
            .declare_subscriber(wildcard)
            .with(flume::bounded(8192))
            .await
            .unwrap();

        let mut tasks = JoinSet::new();
        let (cmd_tx, mut cmd_rx) = mpsc::channel(8192);
        tasks.spawn({
            async move {
                fn handle_command(subs: &mut HashMap<String, mpsc::Sender<Sample>>, cmd: Command) {
                    //println!("Handle Command Called");
                    match cmd {
                        Command::Subscribe(s, tx) => {
                            //println!("Handle Command Subcribe: {}", s);
                            subs.insert(s, tx);
                        }
                        Command::Unsubscribe(s) => {
                            //println!("Handle Command Unsubscribe: {}", s);
                            subs.remove(&s);
                        }
                        Command::Batch(cmds, ack) => {
                            //println!("Handle Command Batch");
                            for cmd in cmds {
                                handle_command(subs, cmd);
                            }
                            let _ = ack.send(());
                        }
                    }
                }

                async fn handle_message(
                    subs: &mut HashMap<String, mpsc::Sender<Sample>>,
                    sample: Sample,
                ) {
                    //println!("Handle Command Called");
                    let key = sample.key_expr().clone().as_str().to_string();
                    // let msg = parse_message(sample);

                    let Some(sub) = subs.get_mut(&key) else {
                        debug!(?key, "drop message with no subscriber");
                        return;
                    };
                    let Ok(sub) = sub.reserve().await else {
                        debug!(?key, "drop message with closed subscriber");
                        subs.remove(&key);
                        return;
                    };

                    sub.send(sample);
                }

                let mut subs = HashMap::new();
                loop {
                    //println!("Loop Called");
                    select! {
                        Ok(msg) = sub.recv_async() => handle_message(&mut subs, msg).await,
                        Some(cmd) = cmd_rx.recv() => handle_command(&mut subs, cmd),
                        else => return,
                    }
                }
            }
        });
        Ok(Self {
            session,
            prefix: prefix.into(),
            inbox: root.into(),
            commands: cmd_tx,
            tasks: Arc::new(tasks),
        })
    }
}

pub struct ByteSubscription(Subscriber);

#[hotpath::measure_all]
impl Stream for ByteSubscription {
    type Item = std::io::Result<Bytes>;

    #[instrument(level = "trace", skip_all)]
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.0.poll_next_unpin(cx) {
            Poll::Ready(Some(sample)) => Poll::Ready(Some(Ok(zbytes_as_bytes(sample.payload())))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[derive(Default)]
enum IndexTrie {
    #[default]
    Empty,
    Leaf(Subscriber),
    IndexNode {
        subscriber: Option<Subscriber>,
        nested: Vec<Option<IndexTrie>>,
    },
    WildcardNode {
        subscriber: Option<Subscriber>,
        nested: Option<Box<IndexTrie>>,
    },
}

#[hotpath::measure_all]
impl<'a> From<(&'a [Option<usize>], Subscriber)> for IndexTrie {
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

impl<P: AsRef<[Option<usize>]>> FromIterator<(P, Subscriber)> for IndexTrie {
    fn from_iter<T: IntoIterator<Item = (P, Subscriber)>>(iter: T) -> Self {
        let mut root = Self::Empty;
        for (path, sub) in iter {
            if !root.insert(path.as_ref(), sub) {
                return Self::Empty;
            }
        }
        root
    }
}

#[hotpath::measure_all]
impl IndexTrie {
    #[inline]
    fn is_empty(&self) -> bool {
        matches!(self, IndexTrie::Empty)
    }

    #[instrument(level = "trace", skip_all)]
    fn take(&mut self, path: &[usize]) -> Option<Subscriber> {
        let Some((i, path)) = path.split_first() else {
            return match mem::take(self) {
                // TODO: Demux the subscription
                //IndexTrie::WildcardNode { subscriber, nested } => {
                //    if let Some(nested) = nested {
                //        *self = IndexTrie::WildcardNode {
                //            subscriber: None,
                //            nested: Some(nested),
                //        }
                //    }
                //    subscriber
                //}
                IndexTrie::Empty | IndexTrie::WildcardNode { .. } => None,
                IndexTrie::Leaf(subscriber) => Some(subscriber),
                IndexTrie::IndexNode { subscriber, nested } => {
                    if !nested.is_empty() {
                        *self = IndexTrie::IndexNode {
                            subscriber: None,
                            nested,
                        }
                    }
                    subscriber
                }
            };
        };
        match self {
            // TODO: Demux the subscription
            //Self::WildcardNode { ref mut nested, .. } => {
            //    nested.as_mut().and_then(|nested| nested.take(path))
            //}
            Self::Empty | Self::Leaf(..) | Self::WildcardNode { .. } => None,
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
                    let cap = i.saturating_add(1);
                    if nested.len() < cap {
                        nested.resize_with(cap, Option::default);
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
    incoming: Option<Subscriber>,
    nested: Arc<std::sync::Mutex<IndexTrie>>,
    path: Box<[usize]>,
}

#[hotpath::measure_all]
impl wrpc_transport::Index<Self> for Reader {
    #[instrument(level = "trace", skip(self))]
    fn index(&self, path: &[usize]) -> anyhow::Result<Self> {
        ensure!(!path.is_empty());
        trace!("locking index tree");
        let mut nested = self
            .nested
            .lock()
            .map_err(|err| anyhow!(err.to_string()).context("failed to lock map"))?;
        trace!("taking index subscription");
        let mut p = self.path.to_vec();
        p.extend_from_slice(path);
        let incoming = nested.take(&p);
        Ok(Self {
            buffer: Bytes::default(),
            incoming,
            nested: Arc::clone(&self.nested),
            path: p.into_boxed_slice(),
        })
    }
}

#[hotpath::measure_all]
impl AsyncRead for Reader {
    #[instrument(level = "trace", skip_all, ret)]
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let cap = buf.remaining();
        if cap == 0 {
            trace!("attempt to read empty buffer");
            return Poll::Ready(Ok(()));
        }

        if !self.buffer.is_empty() {
            if self.buffer.len() > cap {
                trace!(cap, len = self.buffer.len(), "reading part of buffer");
                buf.put_slice(&self.buffer.split_to(cap));
            } else {
                trace!(cap, len = self.buffer.len(), "reading full buffer");
                buf.put_slice(&mem::take(&mut self.buffer));
            }
            return Poll::Ready(Ok(()));
        }
        let Some(incoming) = self.incoming.as_mut() else {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("subscription not found for path {:?}", self.path),
            )));
        };
        trace!("polling for next message");
        match incoming.poll_next_unpin(cx) {
            Poll::Ready(Some(sample)) => {
                let mut payload = zbytes_as_bytes(sample.payload());
                trace!(?payload, "received message");
                if payload.is_empty() {
                    trace!("received stream shutdown message");
                    return Poll::Ready(Ok(()));
                }
                if payload.len() > cap {
                    trace!(len = payload.len(), cap, "partially reading the message");
                    buf.put_slice(&payload.split_to(cap));
                    self.buffer = payload;
                } else {
                    trace!(len = payload.len(), cap, "filling the buffer with payload");
                    buf.put_slice(&payload);
                }
                Poll::Ready(Ok(()))
            }
            Poll::Ready(None) => {
                trace!("subscription finished");
                Poll::Ready(Ok(()))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SubjectWriter {
    session: Arc<zenoh::Session>,
    tx: String,
    shutdown: bool,
    tasks: Arc<JoinSet<()>>,
}

impl SubjectWriter {
    fn new(session: Arc<zenoh::Session>, tx: String, tasks: Arc<JoinSet<()>>) -> Self {
        Self {
            session,
            tx,
            shutdown: false,
            tasks,
        }
    }
}

#[hotpath::measure_all]
impl wrpc_transport::Index<Self> for SubjectWriter {
    #[instrument(level = "trace", skip(self))]
    fn index(&self, path: &[usize]) -> anyhow::Result<Self> {
        ensure!(!path.is_empty());
        let tx = index_path(&self.tx, path);
        Ok(Self {
            session: self.session.clone(),
            tx,
            shutdown: false,
            tasks: Arc::clone(&self.tasks),
        })
    }
}

impl AsyncWrite for SubjectWriter {
    #[instrument(level = "trace", skip_all, ret, fields(subject = self.tx, buf = format!("{buf:02x?}")))]
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        trace!("starting send");

        // let Ok(payload) = prepare_attachment_bytes(self.tx.clone(), None, buf, None, None) else {
        //     return Poll::Ready(Err(io::Error::new(
        //         io::ErrorKind::InvalidData,
        //         "Couldn't prepare payload as bytes",
        //     )));
        // };

        let subject = &self.tx;
        let session = &self.session;

        trace!(
            "writing message synchronously: key={} size={}",
            subject,
            buf.len()
        );

        match send_sync(session, subject, &buf) {
            Ok(()) => {
                trace!("put completed");
                Poll::Ready(Ok(buf.len()))
            }
            Err(e) => {
                warn!(?e, "failed to publish sync put");
                Poll::Ready(Err(e))
            }
        }
    }

    #[instrument(level = "trace", skip_all, ret, fields(subject = self.tx))]
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        trace!("flushing");
        Poll::Ready(Ok(()))
    }

    #[instrument(level = "trace", skip_all, ret, fields(subject = self.tx))]
    fn poll_shutdown(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        trace!("writing stream shutdown message");
        if !self.shutdown {
            let session = self.session.clone();
            let key = self.tx.clone();
            self.shutdown = true;

            spawn_async(async move {
                trace!("writing stream shutdown message (explicit)");
                if let Err(err) = session.put(key, &[]).await {
                    warn!(?err, "failed to publish stream shutdown message");
                }
            });
        }
        Poll::Ready(Ok(()))
    }
}

#[hotpath::measure_all]
impl Drop for SubjectWriter {
    fn drop(&mut self) {
        if !self.shutdown {
            let session = self.session.clone();
            let key = self.tx.clone();
            let tasks = Arc::clone(&self.tasks);
            spawn_async(async move {
                trace!("writing stream shutdown message");
                // let Ok(payload) = prepare_attachment_bytes(key.clone(), None, &[], None, None) else {
                //     warn!("Couldn't prepare payload as bytes");
                //     return ();
                // };
                if let Err(err) = session.put(key, &[]).await {
                    warn!(?err, "failed to publish stream shutdown message");
                }
                drop(tasks);
            });
        }
    }
}

#[derive(Default)]
pub enum RootParamWriter {
    #[default]
    Corrupted,
    Handshaking {
        session: Arc<zenoh::Session>,
        sub: Subscriber,
        indexed: std::sync::Mutex<Vec<(Vec<usize>, oneshot::Sender<SubjectWriter>)>>,
        buffer: Bytes,
        tasks: Arc<JoinSet<()>>,
    },
    Draining {
        tx: SubjectWriter,
        buffer: Bytes,
    },
    Active(SubjectWriter),
}

impl RootParamWriter {
    fn new(
        session: Arc<zenoh::Session>,
        sub: Subscriber,
        buffer: Bytes,
        tasks: Arc<JoinSet<()>>,
    ) -> Self {
        Self::Handshaking {
            session,
            sub,
            indexed: std::sync::Mutex::default(),
            buffer,
            tasks,
        }
    }
}

#[hotpath::measure]
fn map_status_to_error_kind(code: &str) -> Option<io::ErrorKind> {
    let c = code.trim();
    match c {
        // Allow either symbolic names or common numeric equivalents
        c if c.eq_ignore_ascii_case("NO_RESPONDERS") || c == "503" => {
            Some(io::ErrorKind::NotConnected)
        }
        c if c.eq_ignore_ascii_case("TIMEOUT") || c == "408" => Some(io::ErrorKind::TimedOut),
        c if c.eq_ignore_ascii_case("REQUEST_TERMINATED") => Some(io::ErrorKind::UnexpectedEof),
        _ => None,
    }
}

#[hotpath::measure_all]
impl RootParamWriter {
    #[instrument(level = "trace", skip_all, ret)]
    fn poll_active(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut *self {
            Self::Corrupted => Poll::Ready(Err(corrupted_memory_error())),
            Self::Handshaking { sub, .. } => {
                trace!("polling for handshake response 1");
                match sub.poll_next_unpin(cx) {
                    // Poll::Ready(Some(sample) => {
                    //     if let Some(kind) = map_status_to_error_kind(&code) {
                    //         return Poll::Ready(Err(kind.into()));
                    //     }
                    //     if !code.is_empty() {
                    //         let msg = match description {
                    //             Some(desc) if !desc.is_empty() => {
                    //                 format!("received a response with code `{code}` ({desc})")
                    //             }
                    //             _ => format!("received a response with code `{code}`"),
                    //         };
                    //         return Poll::Ready(Err(io::Error::other(msg)));
                    //     }
                    //     // Empty status string: fall through to the generic error below
                    //     return Poll::Ready(Err(io::Error::new(
                    //         io::ErrorKind::InvalidInput,
                    //         "empty status string in handshake response",
                    //     )));
                    // }
                    Poll::Ready(Some(sample)) => {
                        let Self::Handshaking {
                            session,
                            indexed,
                            buffer,
                            tasks,
                            ..
                        } = mem::take(&mut *self)
                        else {
                            return Poll::Ready(Err(corrupted_memory_error()));
                        };

                        let tx = match sample.attachment() {
                            Some(zbytes) => zbytes.try_to_string().unwrap().into_owned(),
                            None => {
                                return Poll::Ready(Err(std::io::Error::new(
                                std::io::ErrorKind::InvalidInput,
                                "peer did not specify a reply subject",
                            )))
                            },
                        };

                        let tx = SubjectWriter::new(session, param_subject(&tx), tasks);
                        let indexed = indexed
                            .into_inner()
                            .map_err(|err| std::io::Error::other(err.to_string()))?;
                        for (path, tx_tx) in indexed {
                            let tx = tx.index(&path).map_err(std::io::Error::other)?;
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

#[hotpath::measure_all]
impl wrpc_transport::Index<IndexedParamWriter> for RootParamWriter {
    #[instrument(level = "trace", skip(self))]
    fn index(&self, path: &[usize]) -> anyhow::Result<IndexedParamWriter> {
        ensure!(!path.is_empty());
        match self {
            Self::Corrupted => Err(anyhow!(corrupted_memory_error())),
            Self::Handshaking { indexed, .. } => {
                let (tx_tx, tx_rx) = oneshot::channel();
                let mut indexed = indexed
                    .lock()
                    .map_err(|err| std::io::Error::other(err.to_string()))?;
                indexed.push((path.to_vec(), tx_tx));
                Ok(IndexedParamWriter::Handshaking {
                    tx_rx,
                    indexed: std::sync::Mutex::default(),
                })
            }
            Self::Draining { tx, .. } | Self::Active(tx) => {
                tx.index(path).map(IndexedParamWriter::Active)
            }
        }
    }
}

#[hotpath::measure_all]
impl AsyncWrite for RootParamWriter {
    #[instrument(level = "trace", skip_all, ret, fields(buf = format!("{buf:02x?}")))]
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

    #[instrument(level = "trace", skip_all, ret)]
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

    #[instrument(level = "trace", skip_all, ret)]
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

#[hotpath::measure_all]
impl IndexedParamWriter {
    #[instrument(level = "trace", skip_all, ret)]
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
                        let indexed = indexed
                            .into_inner()
                            .map_err(|err| std::io::Error::other(err.to_string()))?;
                        for (path, tx_tx) in indexed {
                            let tx = tx.index(&path).map_err(std::io::Error::other)?;
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

#[hotpath::measure_all]
impl wrpc_transport::Index<Self> for IndexedParamWriter {
    #[instrument(level = "trace", skip_all)]
    fn index(&self, path: &[usize]) -> anyhow::Result<Self> {
        ensure!(!path.is_empty());
        match self {
            Self::Corrupted => Err(anyhow!(corrupted_memory_error())),
            Self::Handshaking { indexed, .. } => {
                let (tx_tx, tx_rx) = oneshot::channel();
                let mut indexed = indexed
                    .lock()
                    .map_err(|err| std::io::Error::other(err.to_string()))?;
                indexed.push((path.to_vec(), tx_tx));
                Ok(Self::Handshaking {
                    tx_rx,
                    indexed: std::sync::Mutex::default(),
                })
            }
            Self::Active(tx) => tx.index(path).map(Self::Active),
        }
    }
}

#[hotpath::measure_all]
impl AsyncWrite for IndexedParamWriter {
    #[instrument(level = "trace", skip_all, ret, fields(buf = format!("{buf:02x?}")))]
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

    #[instrument(level = "trace", skip_all, ret)]
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

    #[instrument(level = "trace", skip_all, ret)]
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

pub enum ParamWriter {
    Root(RootParamWriter),
    Nested(IndexedParamWriter),
}

#[hotpath::measure_all]
impl wrpc_transport::Index<Self> for ParamWriter {
    fn index(&self, path: &[usize]) -> anyhow::Result<Self> {
        ensure!(!path.is_empty());
        match self {
            ParamWriter::Root(w) => w.index(path),
            ParamWriter::Nested(w) => w.index(path),
        }
        .map(Self::Nested)
    }
}

#[hotpath::measure_all]
impl AsyncWrite for ParamWriter {
    #[instrument(level = "trace", skip_all, ret, fields(buf = format!("{buf:02x?}")))]
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match &mut *self {
            ParamWriter::Root(w) => pin!(w).poll_write(cx, buf),
            ParamWriter::Nested(w) => pin!(w).poll_write(cx, buf),
        }
    }

    #[instrument(level = "trace", skip_all, ret)]
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut *self {
            ParamWriter::Root(w) => pin!(w).poll_flush(cx),
            ParamWriter::Nested(w) => pin!(w).poll_flush(cx),
        }
    }

    #[instrument(level = "trace", skip_all, ret)]
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut *self {
            ParamWriter::Root(w) => pin!(w).poll_shutdown(cx),
            ParamWriter::Nested(w) => pin!(w).poll_shutdown(cx),
        }
    }
}

impl wrpc_transport::Invoke for Client {
    type Context = ();
    type Outgoing = ParamWriter;
    type Incoming = Reader;

    #[instrument(level = "trace", skip(self, paths, params), fields(params = format!("{params:02x?}")))]
    async fn invoke<P: AsRef<[Option<usize>]> + Send + Sync>(
        &self,
        cx: Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Self::Outgoing, Self::Incoming)> {
        let paths = paths.as_ref();
        let mut cmds = Vec::with_capacity(paths.len().saturating_add(2));

        let rx = child_inbox(&self.inbox);
        let (handshake_tx, handshake_rx) = mpsc::channel(1);
        cmds.push(Command::Subscribe(rx.clone(), handshake_tx));

        let result = result_subject(&rx);
        let (result_tx, result_rx) = mpsc::channel(16);
        cmds.push(Command::Subscribe(result.clone(), result_tx));

        let nested = paths.iter().map(|path| {
            let (tx, rx) = mpsc::channel(16);
            let subject = subscribe_path(&result, path.as_ref());
            cmds.push(Command::Subscribe(subject.clone(), tx));
            Subscriber {
                rx: ReceiverStream::new(rx),
                commands: self.commands.clone(),
                subject,
                tasks: Arc::clone(&self.tasks),
            }
        });
        let nested: IndexTrie = zip(paths.iter(), nested).collect();
        ensure!(
            paths.is_empty() == nested.is_empty(),
            "failed to construct subscription tree"
        );

        let (ack_tx, ack_rx) = oneshot::channel();

        self.commands
            .send(Command::Batch(cmds.into_boxed_slice(), ack_tx))
            .await
            .context("failed to subscribe")?;

        ack_rx.await?;

        let param_tx = invocation_subject(&self.prefix, instance, func);
        trace!("publishing handshake");
        self.session.put(param_tx, params).attachment(rx.clone()).await.unwrap();
        // publish_with_reply(&self.session, param_tx, rx.clone(), params)
        //     .await
        //     .unwrap();

        // let session = Arc::clone(&self.session);
        // tokio::spawn(async move {
        //     if let Err(err) = nats.flush().await {
        //         error!(?err, "failed to flush");
        //     }
        // });
        Ok((
            ParamWriter::Root(RootParamWriter::new(
                (*self.session).clone().into(),
                Subscriber {
                    rx: ReceiverStream::new(handshake_rx),
                    commands: self.commands.clone(),
                    subject: rx,
                    tasks: Arc::clone(&self.tasks),
                },
                Bytes::new(),
                Arc::clone(&self.tasks),
            )),
            Reader {
                buffer: Bytes::default(),
                incoming: Some(Subscriber {
                    rx: ReceiverStream::new(result_rx),
                    commands: self.commands.clone(),
                    subject: result,
                    tasks: Arc::clone(&self.tasks),
                }),
                nested: Arc::new(std::sync::Mutex::new(nested)),
                path: Box::default(),
            },
        ))
    }
}

#[hotpath::measure]
async fn handle_message(
    session: &zenoh::Session,
    rx: String,
    commands: mpsc::Sender<Command>,
    sample: Sample,
    paths: &[Box<[Option<usize>]>],
    tasks: Arc<JoinSet<()>>,
) -> anyhow::Result<((), SubjectWriter, Reader)> {
    let tx = match sample.attachment() {
        Some(zbytes) => zbytes.try_to_string().unwrap().into_owned(),
        None => todo!(),
    };

    let mut cmds = Vec::with_capacity(paths.len().saturating_add(1));

    let param = param_subject(&rx);
    let (param_tx, param_rx) = mpsc::channel(16);
    cmds.push(Command::Subscribe(param.clone(), param_tx));

    let nested = paths.iter().map(|path| {
        let (tx, rx) = mpsc::channel(16);
        let subject = subscribe_path(&param, path.as_ref());
        cmds.push(Command::Subscribe(subject.clone(), tx));
        Subscriber {
            rx: ReceiverStream::new(rx),
            commands: commands.clone(),
            subject,
            tasks: Arc::clone(&tasks),
        }
    });
    let nested: IndexTrie = zip(paths.iter(), nested).collect();
    ensure!(
        paths.is_empty() == nested.is_empty(),
        "failed to construct subscription tree"
    );

    let (ack_tx, ack_rx) = oneshot::channel();

    commands
        .send(Command::Batch(cmds.into_boxed_slice(), ack_tx))
        .await
        .context("failed to subscribe")?;

    ack_rx.await?;

    trace!("publishing handshake response");

    session.put(tx.clone(), Bytes::default()).attachment(rx.clone()).await.unwrap();
    Ok((
        (),
        SubjectWriter::new(
            session.clone().into(),
            result_subject(&tx),
            Arc::clone(&tasks),
        ),
        Reader {
            buffer: zbytes_as_bytes(sample.payload()),
            incoming: Some(Subscriber {
                rx: ReceiverStream::new(param_rx),
                commands,
                subject: param,
                tasks,
            }),
            nested: Arc::new(std::sync::Mutex::new(nested)),
            path: Box::default(),
        },
    ))
}

#[hotpath::measure_all]
impl wrpc_transport::Serve for Client {
    type Context = ();
    type Outgoing = SubjectWriter;
    type Incoming = Reader;

    #[instrument(level = "trace", skip(self, paths))]
    async fn serve(
        &self,
        instance: &str,
        func: &str,
        paths: impl Into<Arc<[Box<[Option<usize>]>]>> + Send,
    ) -> anyhow::Result<
        impl Stream<Item = anyhow::Result<(Self::Context, Self::Outgoing, Self::Incoming)>> + 'static,
    > {
        let subject = invocation_subject(&self.prefix, instance, func);

        //println!("Client serve subscribe to: {}", subject);

        let sub = self.session.declare_subscriber(subject).await.unwrap();
        let session = Arc::clone(&self.session);
        let paths = paths.into();
        let commands = self.commands.clone();
        let inbox_root = self.inbox.clone();
        let tasks = Arc::clone(&self.tasks);

        let stream = futures::stream::unfold(
            (sub, session, paths, commands, tasks, inbox_root),
            |(sub, session, paths, commands, tasks, inbox_root)| async move {
                match sub.recv_async().await {
                    Ok(sample) => {
                        // let Ok(message) = parse_bytes_message(sample) else {
                        //     return None;
                        // };

                        let rx = child_inbox(inbox_root.as_ref());
                        let item = handle_message(
                            &session,
                            rx,
                            commands.clone(),
                            sample,
                            &paths,
                            Arc::clone(&tasks),
                        )
                        .await;

                        // put `inboxroot` back into state for the next loop
                        Some((item, (sub, session, paths, commands, tasks, inbox_root)))
                    }
                    Err(_) => None,
                }
            },
        );

        Ok(stream)
    }
}
