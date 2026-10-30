//! `wrpc:rpc` implementation

use core::fmt;
use core::future;
use core::mem;
use core::pin::Pin;
use core::task::{Context, Poll, ready};

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};

use futures::FutureExt as _;
use futures::future::{BoxFuture, Shared};
use wasmtime::component::{HasData, Linker};
use wasmtime::error::Context as _;
use wrpc_transport::Invoke;
use wrpc_transport::frame::{Incoming, Outgoing};

use crate::{WrpcView, bindings};

mod host;

/// Wrapper struct, for which [`crate::bindings::rpc::transport::Host`] is implemented
#[repr(transparent)]
pub struct WrpcRpcImpl<T>(pub T);

impl<T: 'static> HasData for WrpcRpcImpl<T> {
    type Data<'a> = WrpcRpcImpl<&'a mut T>;
}

pub fn add_to_linker<T>(linker: &mut Linker<T>) -> wasmtime::Result<()>
where
    T: WrpcView + 'static,
    T::Invoke: Clone + 'static,
    <T::Invoke as Invoke>::Context: 'static,
{
    bindings::rpc::context::add_to_linker::<_, WrpcRpcImpl<T>>(linker, |t| WrpcRpcImpl(t))
        .context("failed to link `wrpc:rpc/context`")?;
    bindings::rpc::error::add_to_linker::<_, WrpcRpcImpl<T>>(linker, |t| WrpcRpcImpl(t))
        .context("failed to link `wrpc:rpc/error`")?;
    bindings::rpc::invoker::add_to_linker::<_, WrpcRpcImpl<T>>(linker, |t| WrpcRpcImpl(t))
        .context("failed to link `wrpc:rpc/invoker`")?;
    bindings::rpc::transport::add_to_linker::<_, WrpcRpcImpl<T>>(linker, |t| WrpcRpcImpl(t))
        .context("failed to link `wrpc:rpc/transport`")?;
    Ok(())
}

/// RPC error
pub enum Error {
    /// Failure of an invocation
    Invoke(anyhow::Error),
    /// Failure to transmit an invocation
    Transmission(Arc<anyhow::Error>),
    /// Error originating from an `index` call on an incoming channel
    IncomingIndex(std::io::Error),
    /// Error originating from an `index` call on an outgoing channel
    OutgoingIndex(std::io::Error),
    /// Error originating from reading the data of an incoming channel
    Read(std::io::Error),
    /// Error originating from writing the data of an outgoing channel
    Write(std::io::Error),
    /// The data of a channel has already been taken
    DataTaken,
    /// The channel is not at the position of a declared async value
    Undeclared,
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Invoke(error) => error.fmt(f),
            Error::Transmission(error) => {
                "invocation was not transmitted: ".fmt(f)?;
                error.fmt(f)
            }
            Error::IncomingIndex(error)
            | Error::OutgoingIndex(error)
            | Error::Read(error)
            | Error::Write(error) => error.fmt(f),
            Error::DataTaken => "channel data already taken".fmt(f),
            Error::Undeclared => "channel is not at the position of a declared async value".fmt(f),
        }
    }
}

/// Transmission of an invocation, which is driven by whichever of its channels or its result is
/// polled first, setting the roots of both directions of the invocation
type Transmission = Shared<BoxFuture<'static, Result<(), Arc<anyhow::Error>>>>;

/// Handle of the position of an incoming channel, set once the invocation is transmitted
type Slot = Mutex<Option<std::io::Result<Incoming>>>;

/// Root of the incoming direction of an invocation, owned by its channels
#[derive(Default)]
struct IncomingRoot {
    /// Root handle, set once the invocation is transmitted
    root: OnceLock<Mutex<Incoming>>,
    /// Channels returned before the invocation was transmitted
    pending: Mutex<PendingChannels>,
}

/// Position of a channel returned before the invocation was transmitted
struct Pending {
    path: Box<[usize]>,
    /// Pattern matching `path`, with `None` at the positions matched by a wildcard
    pattern: Box<[Option<usize>]>,
    slot: Weak<Slot>,
    /// Whether the position is that of a declared async value, whose data has to be taken even
    /// if the channel was dropped, so that it is discarded
    declared: bool,
}

impl Pending {
    /// Returns the paths of the list and stream elements containing the position, outermost first
    fn elements(&self) -> impl Iterator<Item = &[usize]> {
        self.pattern
            .iter()
            .enumerate()
            .filter(|(.., p)| p.is_none())
            .map(|(k, ..)| &self.path[..=k])
    }
}

/// Channels returned before the invocation was transmitted, which mirror the elements the
/// transport releases, so that indexing behaves the same before and after transmission
#[derive(Default)]
struct PendingChannels {
    /// Positions of the channels, which are alive or declared, in the order they were returned
    channels: Vec<Pending>,
    /// Index following the last element a channel was returned within, keyed by the path of
    /// the list or stream containing it
    next: BTreeMap<Box<[usize]>, usize>,
    /// Channels returned within an element, which may still be alive, keyed by its path
    elements: BTreeMap<Box<[usize]>, Vec<Weak<Slot>>>,
    /// Declared positions in `channels`
    declared: BTreeSet<Box<[usize]>>,
    /// Number of `channels`, at which the dropped ones are pruned next
    prune_at: usize,
}

impl PendingChannels {
    /// Returns whether the element at `path` is released, see [`is_released`]
    fn is_released(&self, path: &[usize]) -> bool {
        is_released(&self.next, &self.elements, path)
    }

    /// Records the position of a channel, failing if it is within a released element or a
    /// declared position, whose channel was already returned
    fn push(&mut self, entry: Pending) -> std::io::Result<()> {
        if entry.elements().any(|element| self.is_released(element)) {
            return Err(error_discarded(&entry.path));
        }
        if entry.declared && !self.declared.insert(entry.path.clone()) {
            return Err(error_returned(&entry.path));
        }
        for element in entry.elements() {
            let (i, parent) = element.split_last().expect("element path is not empty");
            let next = self.next.entry(Box::from(parent)).or_default();
            *next = (*next).max(i.saturating_add(1));
            self.elements
                .entry(Box::from(element))
                .or_default()
                .push(Weak::clone(&entry.slot));
        }
        self.channels.push(entry);
        if self.channels.len() >= self.prune_at {
            self.prune();
            self.prune_at = self.channels.len().saturating_mul(2).max(64);
        }
        Ok(())
    }

    /// Forgets the dropped channels, which need not be indexed, and the elements released
    fn prune(&mut self) {
        self.elements.retain(|_, slots| {
            slots.retain(|slot| slot.strong_count() > 0);
            !slots.is_empty()
        });
        let released: Vec<_> = self
            .next
            .keys()
            .filter(|parent| (1..=parent.len()).any(|k| self.is_released(&parent[..k])))
            .cloned()
            .collect();
        for parent in released {
            self.next.remove(&parent);
        }
        let (next, elements) = (&self.next, &self.elements);
        self.channels.retain(|entry| {
            entry.slot.strong_count() > 0
                || entry.declared
                    && !entry
                        .elements()
                        .any(|element| is_released(next, elements, element))
        });
        self.declared = self
            .channels
            .iter()
            .filter(|Pending { declared, .. }| *declared)
            .map(|Pending { path, .. }| path.clone())
            .collect();
    }
}

/// Returns whether the element at `path` is released, i.e. a channel was returned within it or
/// a following element and no channel returned within it is alive
fn is_released(
    next: &BTreeMap<Box<[usize]>, usize>,
    elements: &BTreeMap<Box<[usize]>, Vec<Weak<Slot>>>,
    path: &[usize],
) -> bool {
    let Some((i, parent)) = path.split_last() else {
        return false;
    };
    next.get(parent).is_some_and(|next| i < next)
        && !elements
            .get(path)
            .is_some_and(|slots| slots.iter().any(|slot| slot.strong_count() > 0))
}

impl IncomingRoot {
    /// Sets the root handle, indexing the positions of the channels returned before in the
    /// order they were returned and releasing the elements released before
    fn set(&self, incoming: Incoming) {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        let released: Vec<_> = pending
            .next
            .iter()
            .map(|(parent, next)| [parent, &[next - 1][..]].concat())
            .filter(|element| pending.is_released(element))
            .collect();
        let mut dropped = Vec::default();
        for Pending { path, slot, .. } in mem::take(&mut pending.channels) {
            let handle = incoming.index(&path);
            if let Some(slot) = slot.upgrade() {
                *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(handle);
            } else {
                dropped.push(handle);
            }
        }
        for element in released {
            _ = incoming.index(&element);
        }
        drop(dropped);
        *pending = PendingChannels::default();
        _ = self.root.set(Mutex::new(incoming));
    }
}

/// Takes the reader of the handle in `slot`, if the handle is set
fn take_reader(slot: &Slot) -> Option<std::io::Result<Incoming>> {
    let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
    match slot.as_mut()? {
        Ok(incoming) => Some(incoming.take_reader().ok_or_else(error_data_taken)),
        Err(..) => slot.take(),
    }
}

/// Error returned for a channel, whose data was already taken
fn error_data_taken() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::AlreadyExists, "data already taken")
}

/// Failure to transmit an invocation, shared by all of its channels and its result
#[derive(Debug)]
struct NotTransmitted(Arc<anyhow::Error>);

impl fmt::Display for NotTransmitted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        "invocation was not transmitted: ".fmt(f)?;
        self.0.fmt(f)
    }
}

impl std::error::Error for NotTransmitted {}

/// Error returned for the channels of an invocation, which failed to be transmitted
fn error_not_transmitted(err: &Arc<anyhow::Error>) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::NotConnected,
        NotTransmitted(Arc::clone(err)),
    )
}

/// Error returned for the channels of an invocation, whose direction ended before it was
/// transmitted
fn error_ended() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::NotConnected,
        "invocation direction ended before transmission",
    )
}

/// Handle of a channel, which is available once the invocation is transmitted
enum Deferred<T> {
    Pending(Pin<Box<dyn Future<Output = std::io::Result<T>> + Send>>),
    Ready(T),
}

impl<T> Deferred<T> {
    fn new(fut: impl Future<Output = std::io::Result<T>> + Send + 'static) -> Self {
        Self::Pending(Box::pin(fut))
    }

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<std::io::Result<&mut T>> {
        match self {
            Self::Pending(fut) => {
                let handle = ready!(fut.as_mut().poll(cx))?;
                *self = Self::Ready(handle);
                self.poll_ready(cx)
            }
            Self::Ready(handle) => Poll::Ready(Ok(handle)),
        }
    }
}

/// Declared path or a prefix of one
#[derive(Debug, Default)]
struct Pattern {
    /// Whether the path is declared
    declared: bool,
    /// Whether the positions nested in the path are matched by a wildcard, if there are any
    wildcard: Option<bool>,
    /// Whether the channel of the declared path was returned, tracked for paths without
    /// wildcards only
    returned: bool,
}

/// Incoming direction of an invocation, shared by its channels
struct IncomingInvocation {
    /// Transmission of the invocation
    transmission: Transmission,
    root: Arc<IncomingRoot>,
    /// Declared paths and their prefixes
    patterns: Mutex<BTreeMap<Box<[Option<usize>]>, Pattern>>,
}

impl IncomingInvocation {
    /// Returns a future resolving to the result of the incoming direction once it ends, which
    /// keeps the direction open only until the invocation is transmitted
    fn finished(&self) -> BoxFuture<'static, std::io::Result<()>> {
        if let Some(incoming) = self.root.root.get() {
            return incoming
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .finished()
                .boxed();
        }
        let transmission = self.transmission.clone();
        let root = Arc::clone(&self.root);
        async move {
            transmission
                .await
                .map_err(|err| error_not_transmitted(&err))?;
            let finished = root
                .root
                .get()
                .ok_or_else(error_ended)?
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .finished();
            drop(root);
            finished.await
        }
        .boxed()
    }
}

/// `wrpc:rpc/transport.incoming-channel` representation
pub struct IncomingChannel {
    invocation: Arc<IncomingInvocation>,
    path: Box<[usize]>,
    /// Handle of the position, which is not set for the root
    incoming: Arc<Slot>,
    /// Whether this is the root channel or the channel of a declared position
    declared: bool,
    data_taken: bool,
}

impl IncomingChannel {
    fn new<P: AsRef<[Option<usize>]>>(
        transmission: Transmission,
        root: Arc<IncomingRoot>,
        paths: impl IntoIterator<Item = P>,
    ) -> Self {
        let mut patterns = BTreeMap::from([(
            Box::default(),
            Pattern {
                declared: true,
                ..Pattern::default()
            },
        )]);
        for path in paths {
            let path = path.as_ref();
            for (k, i) in path.iter().enumerate() {
                patterns.entry(Box::from(&path[..k])).or_default().wildcard = Some(i.is_none());
            }
            patterns.entry(Box::from(path)).or_default().declared = true;
        }
        Self {
            invocation: Arc::new(IncomingInvocation {
                transmission,
                root,
                patterns: Mutex::new(patterns),
            }),
            path: Box::default(),
            incoming: Arc::default(),
            declared: true,
            data_taken: false,
        }
    }

    fn index(&self, path: &[usize]) -> std::io::Result<Self> {
        if path.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "path cannot be empty",
            ));
        }
        let path: Box<[usize]> = [&self.path, path].concat().into();
        let mut patterns = self
            .invocation
            .patterns
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut pattern = Vec::with_capacity(path.len());
        for i in &path {
            let wildcard = patterns
                .get(pattern.as_slice())
                .and_then(|node| node.wildcard)
                .ok_or_else(|| error_undeclared(&path))?;
            pattern.push(if wildcard { None } else { Some(*i) });
        }
        let node = patterns
            .get_mut(pattern.as_slice())
            .ok_or_else(|| error_undeclared(&path))?;
        let concrete = pattern.iter().all(Option::is_some);
        if node.declared && concrete && node.returned {
            return Err(error_returned(&path));
        }
        let incoming = Arc::<Slot>::default();
        {
            let mut pending = self
                .invocation
                .root
                .pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(root) = self.invocation.root.root.get() {
                let handle = root
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .index(&path)?;
                if node.declared && !handle.is_subscribed() {
                    return Err(error_returned(&path));
                }
                *incoming.lock().unwrap_or_else(PoisonError::into_inner) = Some(Ok(handle));
            } else if let Err(err) = pending.push(Pending {
                path: path.clone(),
                pattern: pattern.into(),
                slot: Arc::downgrade(&incoming),
                declared: node.declared,
            }) {
                *incoming.lock().unwrap_or_else(PoisonError::into_inner) = Some(Err(err));
            }
        }
        if node.declared && concrete {
            node.returned = true;
        }
        Ok(Self {
            invocation: Arc::clone(&self.invocation),
            declared: node.declared,
            path,
            incoming,
            data_taken: false,
        })
    }

    /// Takes the data of this channel, returning the handle reading it
    fn take_data(&mut self) -> Result<Deferred<Incoming>, Error> {
        if self.data_taken {
            return Err(Error::DataTaken);
        }
        if !self.declared {
            return Err(Error::Undeclared);
        }
        self.data_taken = true;
        if self.path.is_empty() {
            let invocation = Arc::clone(&self.invocation);
            return Ok(Deferred::new(async move {
                invocation
                    .transmission
                    .clone()
                    .await
                    .map_err(|err| error_not_transmitted(&err))?;
                let root = invocation.root.root.get().ok_or_else(error_ended)?;
                root.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take_reader()
                    .ok_or_else(error_data_taken)
            }));
        }
        if let Some(res) = take_reader(&self.incoming) {
            return Ok(Deferred::new(future::ready(res)));
        }
        let invocation = Arc::clone(&self.invocation);
        let incoming = Arc::clone(&self.incoming);
        Ok(Deferred::new(async move {
            invocation
                .transmission
                .clone()
                .await
                .map_err(|err| error_not_transmitted(&err))?;
            take_reader(&incoming).unwrap_or_else(|| Err(error_ended()))
        }))
    }
}

/// Returns the error for a `path` within a released list or stream element
fn error_discarded(path: &[usize]) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("`{path:?}` is within a discarded element"),
    )
}

/// Returns the error for a declared `path`, whose channel was already returned
fn error_returned(path: &[usize]) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        format!("`{path:?}` channel already returned"),
    )
}

/// Returns the error for a `path`, which is neither declared nor a prefix of a declared one
fn error_undeclared(path: &[usize]) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("`{path:?}` is not a declared position"),
    )
}

/// `wrpc:rpc/transport.outgoing-channel` representation
pub struct OutgoingChannel {
    /// Transmission of the invocation
    transmission: Transmission,
    /// Root handle of the outgoing direction, set once the invocation is transmitted
    root: Arc<OnceLock<Outgoing>>,
    path: Box<[usize]>,
    data_taken: bool,
}

impl OutgoingChannel {
    fn new(transmission: Transmission, root: Arc<OnceLock<Outgoing>>) -> Self {
        Self {
            transmission,
            root,
            path: Box::default(),
            data_taken: false,
        }
    }

    fn index(&self, path: &[usize]) -> std::io::Result<Self> {
        if path.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "path cannot be empty",
            ));
        }
        Ok(Self {
            transmission: self.transmission.clone(),
            root: Arc::clone(&self.root),
            path: [&self.path, path].concat().into(),
            data_taken: false,
        })
    }

    /// Returns a future resolving to the handle writing to the position of this channel
    fn outgoing(&self) -> impl Future<Output = std::io::Result<Outgoing>> + Send + 'static + use<> {
        let transmission = self.transmission.clone();
        let root = Arc::clone(&self.root);
        let path = self.path.clone();
        async move {
            transmission
                .await
                .map_err(|err| error_not_transmitted(&err))?;
            let root = root.get().ok_or_else(error_ended)?;
            if path.is_empty() {
                Ok(root.clone())
            } else {
                root.index(&path)
            }
        }
    }

    /// Takes the data of this channel, returning a future resolving to the handle writing it
    fn take_data(
        &mut self,
    ) -> Result<impl Future<Output = std::io::Result<Outgoing>> + Send + 'static + use<>, Error>
    {
        if self.data_taken {
            return Err(Error::DataTaken);
        }
        self.data_taken = true;
        Ok(self.outgoing())
    }
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;

    use bytes::BytesMut;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _, BufReader, DuplexStream};
    use tokio::sync::oneshot;
    use tokio_util::codec::Encoder as _;
    use wrpc_transport::{FrameEncoder, FrameRef};

    use super::*;

    fn frame(path: &[usize], data: &[u8]) -> BytesMut {
        let mut buf = BytesMut::default();
        FrameEncoder
            .encode(FrameRef { path, data }, &mut buf)
            .unwrap();
        buf
    }

    fn incoming<P: AsRef<[Option<usize>]>>(
        paths: impl IntoIterator<Item = P>,
    ) -> (DuplexStream, Incoming) {
        let (tx, rx) = tokio::io::duplex(64);
        (tx, Incoming::new(BufReader::new(rx), paths))
    }

    fn transmission() -> (oneshot::Sender<()>, Transmission) {
        let (tx, rx) = oneshot::channel::<()>();
        let fut = async move {
            _ = rx.await;
            Ok(())
        };
        (tx, fut.boxed().shared())
    }

    fn ready_transmission() -> Transmission {
        future::ready(Ok(())).boxed().shared()
    }

    async fn ready(incoming: Deferred<Incoming>) -> std::io::Result<Incoming> {
        match incoming {
            Deferred::Pending(fut) => fut.await,
            Deferred::Ready(incoming) => Ok(incoming),
        }
    }

    async fn read(incoming: Deferred<Incoming>) -> std::io::Result<Vec<u8>> {
        let mut incoming = ready(incoming).await?;
        let mut buf = Vec::default();
        incoming.read_to_end(&mut buf).await?;
        Ok(buf)
    }

    #[test_log::test(tokio::test)]
    async fn incoming_channel() {
        for transmitted in [true, false] {
            let paths = [[None, Some(0)], [None, Some(1)]];
            let (mut tx, incoming) = incoming(paths);
            let root = Arc::<IncomingRoot>::default();
            let (transmit, transmission) = transmission();
            let mut incoming = Some(incoming);
            if transmitted {
                root.set(incoming.take().unwrap());
            }
            let rx = IncomingChannel::new(transmission, Arc::clone(&root), paths);
            let first = rx.index(&[0]).unwrap();
            let second = rx.index(&[1]).unwrap();
            let a = first.index(&[0]).unwrap().take_data().unwrap();
            let b = second.index(&[0]).unwrap().take_data().unwrap();
            if let Some(incoming) = incoming {
                root.set(incoming);
            }
            transmit.send(()).unwrap();
            tx.write_all(&frame(&[1, 0], b"\x2b")).await.unwrap();
            tx.write_all(&frame(&[0, 0], b"\x2a")).await.unwrap();
            tx.write_all(&frame(&[0, 1], b"\x2c")).await.unwrap();
            drop(tx);
            assert_eq!(read(b).await.unwrap(), b"\x2b");
            assert_eq!(read(a).await.unwrap(), b"\x2a");
            let c = first.index(&[1]).unwrap().take_data().unwrap();
            assert_eq!(read(c).await.unwrap(), b"\x2c");
        }
    }

    #[test_log::test(tokio::test)]
    async fn incoming_channel_index() {
        fn index_err(rx: &IncomingChannel, path: &[usize]) -> ErrorKind {
            let Err(err) = rx.index(path) else {
                panic!("indexing `{path:?}` should fail");
            };
            err.kind()
        }

        {
            let rx = IncomingChannel::new(
                future::pending().boxed().shared(),
                Arc::default(),
                [&[Some(0)][..], &[Some(1), None, Some(0)]],
            );
            assert!(rx.index(&[0]).unwrap().declared);
            assert_eq!(index_err(&rx, &[0]), ErrorKind::AlreadyExists);
            let mut prefix = rx.index(&[1]).unwrap();
            assert!(matches!(prefix.take_data(), Err(Error::Undeclared)));
            let mut element = prefix.index(&[5, 0]).unwrap();
            assert!(element.declared);
            assert!(element.take_data().is_ok());
            assert!(matches!(element.take_data(), Err(Error::DataTaken)));
            for path in [&[2][..], &[0, 0], &[1, 5, 1]] {
                assert_eq!(index_err(&rx, path), ErrorKind::NotFound);
            }
        }
        {
            let paths = [&[Some(0), None][..], &[Some(0), None, None]];
            let (_tx, incoming) = incoming(paths);
            let root = Arc::<IncomingRoot>::default();
            root.set(incoming);
            let rx = IncomingChannel::new(ready_transmission(), root, paths);
            let element = rx.index(&[0, 3]).unwrap();
            assert_eq!(index_err(&rx, &[0, 3]), ErrorKind::AlreadyExists);
            assert!(element.index(&[0]).unwrap().declared);
        }
        {
            let paths = [
                &[Some(0), None][..],
                &[Some(0), None, None],
                &[Some(1), None],
                &[Some(1), None, None],
            ];
            for transmitted in [true, false] {
                let (_tx, incoming) = incoming(paths);
                let mut incoming = Some(incoming);
                let root = Arc::<IncomingRoot>::default();
                if transmitted {
                    root.set(incoming.take().unwrap());
                }
                let rx = IncomingChannel::new(ready_transmission(), Arc::clone(&root), paths);
                let _nested = rx.index(&[0, 1, 0]).unwrap();
                let returned = rx.index(&[1, 1]).unwrap();
                let _nested_returned = rx.index(&[1, 1, 0]).unwrap();
                drop(returned);
                let again = rx.index(&[1, 1]);
                if let Some(incoming) = incoming {
                    root.set(incoming);
                }
                let mut element = rx.index(&[0, 1]).unwrap();
                let res = ready(element.take_data().unwrap()).await;
                assert!(res.is_ok());
                let err = match again {
                    Ok(mut ch) => {
                        let Err(err) = ready(ch.take_data().unwrap()).await else {
                            panic!("reading data should fail");
                        };
                        err
                    }
                    Err(err) => err,
                };
                assert_eq!(
                    err.kind(),
                    ErrorKind::AlreadyExists,
                    "transmitted: {transmitted}"
                );
            }
        }
        {
            for transmitted in [true, false] {
                let (_tx, incoming) = incoming([[None]]);
                let mut incoming = Some(incoming);
                let root = Arc::<IncomingRoot>::default();
                if transmitted {
                    root.set(incoming.take().unwrap());
                }
                let rx = IncomingChannel::new(ready_transmission(), Arc::clone(&root), [[None]]);
                let mut alive = rx.index(&[2]).unwrap();
                drop(rx.index(&[5]).unwrap());
                let released = rx.index(&[3]);
                if let Some(incoming) = incoming {
                    root.set(incoming);
                }
                let err = match released {
                    Ok(mut ch) => {
                        let Err(err) = ready(ch.take_data().unwrap()).await else {
                            panic!("reading data should fail");
                        };
                        err
                    }
                    Err(err) => err,
                };
                assert_eq!(err.kind(), ErrorKind::NotFound);
                let res = ready(alive.take_data().unwrap()).await;
                assert!(res.is_ok());
            }
        }
    }

    #[test_log::test(tokio::test)]
    async fn incoming_channel_outlives_handles() {
        {
            let paths = [[Some(0)]];
            let (mut tx, incoming) = incoming(paths);
            let root = Arc::<IncomingRoot>::default();
            let weak = Arc::downgrade(&root);
            let rx = IncomingChannel::new(ready_transmission(), root, paths);
            let data = rx.index(&[0]).unwrap().take_data().unwrap();
            drop(rx);
            weak.upgrade().unwrap().set(incoming);
            tx.write_all(&frame(&[0], b"\x2a")).await.unwrap();
            drop(tx);
            assert_eq!(read(data).await.unwrap(), b"\x2a");
        }
        {
            for transmitted in [true, false] {
                let paths: [[Option<usize>; 0]; 0] = [];
                let (mut tx, incoming) = incoming(paths);
                let root = Arc::<IncomingRoot>::default();
                let (transmit, transmission) = transmission();
                let mut pending = Some((incoming, transmit));
                let mut transmit = || {
                    if let Some((incoming, tx)) = pending.take() {
                        root.set(incoming);
                        tx.send(()).unwrap();
                    }
                };
                if transmitted {
                    transmit();
                }
                let mut rx = IncomingChannel::new(transmission, Arc::clone(&root), paths);
                let finished = rx.invocation.finished();
                drop(rx.take_data().unwrap());
                drop(rx);
                transmit();
                drop(root);
                tx.write_all(&frame(&[], b"\x2a")).await.unwrap();
                drop(tx);
                let res = finished.await;
                assert!(res.is_err());
            }
        }
    }
}
