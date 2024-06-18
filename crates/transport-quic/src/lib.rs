use core::borrow::Borrow;
use core::fmt::Display;
use core::future::Future as _;
use core::net::SocketAddr;
use core::pin::{pin, Pin};
use core::task::{ready, Context, Poll};
use core::{mem, str};

use std::collections::{hash_map, HashMap};
use std::sync::Arc;

use anyhow::{bail, ensure, Context as _};
use bytes::{Buf as _, BufMut as _, Bytes, BytesMut};
use futures::{Stream, StreamExt};
use pin_project_lite::pin_project;
use quinn::crypto::rustls::HandshakeData;
use quinn::{Connection, ConnectionError, Endpoint, RecvStream, SendStream, VarInt};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;
use tokio::{spawn, try_join};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::codec::Encoder;
use tracing::{debug, instrument, trace, warn, Instrument as _};
use wasm_tokio::cm::AsyncReadValue as _;
use wasm_tokio::{AsyncReadCore as _, AsyncReadLeb128 as _, CoreNameEncoder, Leb128Encoder};

pub const PROTOCOL: u8 = 0;

/// QUIC invocation
pub type Invocation = wrpc_transport::Invocation<Outgoing, Incoming, Session>;

fn san(instance: &str, func: &str) -> String {
    let instance = instance.replace(':', "_").replace('/', "__");
    format!("{func}.{instance}.server.wrpc")
}

#[derive(Default)]
pub enum IndexTree {
    #[default]
    Empty,
    Leaf {
        tx: Option<oneshot::Sender<RecvStream>>,
        rx: Option<oneshot::Receiver<RecvStream>>,
    },
    IndexNode {
        tx: Option<oneshot::Sender<RecvStream>>,
        rx: Option<oneshot::Receiver<RecvStream>>,
        nested: Vec<Option<IndexTree>>,
    },
    // TODO: Add partially-indexed `WildcardIndexNode`
    WildcardNode {
        tx: Option<oneshot::Sender<RecvStream>>,
        rx: Option<oneshot::Receiver<RecvStream>>,
        nested: Option<Box<IndexTree>>,
    },
}

impl<'a>
    From<(
        &'a [Option<usize>],
        Option<oneshot::Sender<RecvStream>>,
        Option<oneshot::Receiver<RecvStream>>,
    )> for IndexTree
{
    fn from(
        (path, tx, rx): (
            &'a [Option<usize>],
            Option<oneshot::Sender<RecvStream>>,
            Option<oneshot::Receiver<RecvStream>>,
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
        oneshot::Sender<RecvStream>,
        oneshot::Receiver<RecvStream>,
    )> for IndexTree
{
    fn from(
        (path, tx, rx): (
            &'a [Option<usize>],
            oneshot::Sender<RecvStream>,
            oneshot::Receiver<RecvStream>,
        ),
    ) -> Self {
        Self::from((path, Some(tx), Some(rx)))
    }
}

impl<'a> From<(&'a [Option<usize>], oneshot::Sender<RecvStream>)> for IndexTree {
    fn from((path, tx): (&'a [Option<usize>], oneshot::Sender<RecvStream>)) -> Self {
        Self::from((path, Some(tx), None))
    }
}

impl<'a, P: Borrow<&'a [Option<usize>]>> FromIterator<P> for IndexTree {
    fn from_iter<T: IntoIterator<Item = P>>(iter: T) -> Self {
        let mut root = Self::Empty;
        for path in iter {
            let (tx, rx) = oneshot::channel();
            if !root.insert(path.borrow(), Some(tx), Some(rx)) {
                return Self::Empty;
            }
        }
        root
    }
}

impl IndexTree {
    #[instrument(level = "trace", skip_all)]
    fn take_rx(&mut self, path: &[usize]) -> Option<oneshot::Receiver<RecvStream>> {
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

    #[instrument(level = "trace", skip_all)]
    fn take_tx(&mut self, path: &[usize]) -> Option<oneshot::Sender<RecvStream>> {
        let Some((i, path)) = path.split_first() else {
            return match self {
                Self::Empty => None,
                Self::Leaf { tx, .. } => tx.take(),
                Self::IndexNode { tx, rx, nested } => {
                    let tx = tx.take();
                    if nested.is_empty() && rx.is_none() {
                        *self = Self::Empty;
                    }
                    tx
                }
                Self::WildcardNode { tx, rx, nested } => {
                    let tx = tx.take();
                    if nested.is_none() && rx.is_none() {
                        *self = Self::Empty;
                    }
                    tx
                }
            };
        };
        match self {
            Self::Empty | Self::Leaf { .. } | Self::WildcardNode { .. } => None,
            Self::IndexNode { ref mut nested, .. } => nested
                .get_mut(*i)
                .and_then(|nested| nested.as_mut().and_then(|nested| nested.take_tx(path))),
            // TODO: Demux the subscription
            //Self::WildcardNode { ref mut nested, .. } => {
            //    nested.as_mut().and_then(|nested| nested.take(path))
            //}
        }
    }

    /// Inserts `sender` and `receiver` under a `path` - returns `false` if it failed and `true` if it succeeded.
    /// Tree state after `false` is returned is undefined
    #[instrument(level = "trace", skip_all)]
    fn insert(
        &mut self,
        path: &[Option<usize>],
        sender: Option<oneshot::Sender<RecvStream>>,
        receiver: Option<oneshot::Receiver<RecvStream>>,
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
                    *self = Self::IndexNode { tx, rx, nested };
                } else {
                    *self = Self::WildcardNode {
                        tx,
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
                    *tx = sender;
                    *rx = receiver;
                    true
                }
                (_, _, [Some(i), path @ ..]) => {
                    if nested.len() < *i {
                        nested.resize_with(i.saturating_add(1), Option::default);
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
                    *tx = sender;
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

#[derive(Default)]
pub struct Server(Mutex<HashMap<String, mpsc::Sender<Connection>>>);

#[derive(Debug)]
pub enum AcceptError {
    Connection(ConnectionError),
    InvalidData,
    ServerNameMissing,
    UnhandledName(String),
    SendError(mpsc::error::SendError<Connection>),
}

impl Display for AcceptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AcceptError::Connection(err) => err.fmt(f),
            AcceptError::InvalidData => write!(f, "unexpected handshake data type"),
            AcceptError::ServerNameMissing => write!(f, "server name missing in handshake data"),
            AcceptError::UnhandledName(name) => {
                write!(f, "SAN `{name}` does not have a handler registered")
            }
            AcceptError::SendError(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for AcceptError {}

impl Server {
    /// Accept a connection on an endpoint.
    /// Returns `Ok(false)` if the endpoint is closed.
    ///
    /// # Errors
    ///
    /// Returns an error if accepting the connection has failed
    pub async fn accept(&self, endpoint: &quinn::Endpoint) -> Result<bool, AcceptError> {
        let Some(conn) = endpoint.accept().await else {
            return Ok(false);
        };
        let mut conn = conn.accept().map_err(AcceptError::Connection)?;
        let data = conn
            .handshake_data()
            .await
            .map_err(AcceptError::Connection)?;
        let data = data
            .downcast::<HandshakeData>()
            .map_err(|_| AcceptError::InvalidData)?;
        let name = data.server_name.ok_or(AcceptError::ServerNameMissing)?;
        let tx = self.0.lock().await;
        let tx = tx
            .get(&name)
            .ok_or_else(|| AcceptError::UnhandledName(name))?;
        let conn = conn.await.map_err(AcceptError::Connection)?;
        tx.send(conn).await.map_err(AcceptError::SendError)?;
        Ok(true)
    }
}

pub struct Client {
    endpoint: Endpoint,
    addr: SocketAddr,
}

impl Client {
    pub fn new(endpoint: Endpoint, addr: impl Into<SocketAddr>) -> Self {
        Self {
            endpoint,
            addr: addr.into(),
        }
    }
}

pin_project! {
    #[project = IncomingProj]
    pub enum Incoming {
        Accepting {
            index: Arc<std::sync::Mutex<IndexTree>>,
            path: Arc<[usize]>,
            #[pin]
            rx: oneshot::Receiver<RecvStream>,
        },
        Active {
            index: Arc<std::sync::Mutex<IndexTree>>,
            path: Arc<[usize]>,
            #[pin]
            rx: RecvStream,
        },
    }
}

impl wrpc_transport::Index<Self> for Incoming {
    fn index(&self, path: &[usize]) -> anyhow::Result<Self> {
        match self {
            Self::Accepting {
                index, path: base, ..
            }
            | Self::Active {
                index, path: base, ..
            } => {
                let path = if base.is_empty() {
                    Arc::from(path)
                } else {
                    Arc::from([base.as_ref(), path].concat())
                };
                trace!("locking index tree");
                let mut lock = index.lock().map_err(|err| {
                    std::io::Error::new(std::io::ErrorKind::Other, err.to_string())
                })?;
                let rx = lock.take_rx(&path).ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("`{path:?}` subscription not found"),
                    )
                })?;
                Ok(Self::Accepting {
                    index: Arc::clone(index),
                    path,
                    rx,
                })
            }
        }
    }
}

impl AsyncRead for Incoming {
    #[instrument(level = "trace", skip_all)]
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.as_mut().project() {
            IncomingProj::Accepting {
                index,
                path,
                mut rx,
            } => {
                trace!("polling channel");
                let rx = ready!(rx.as_mut().poll(cx))
                    .map_err(|err| std::io::Error::new(std::io::ErrorKind::BrokenPipe, err))?;
                *self = Self::Active {
                    index: Arc::clone(index),
                    path: Arc::clone(path),
                    rx,
                };
                self.poll_read(cx, buf)
            }
            IncomingProj::Active { rx, .. } => {
                trace!("reading buffer");
                AsyncRead::poll_read(rx, cx, buf)
            }
        }
    }
}

pin_project! {
    #[project = OutgoingProj]
    pub enum Outgoing {
        Opening {
            header: Bytes,
            path: Arc<[usize]>,
            #[pin]
            conn: Connection,
        },
        Active {
            header: Bytes,
            path: Arc<[usize]>,
            #[pin]
            conn: Connection,
            #[pin]
            tx: SendStream,
        },
    }
}

impl wrpc_transport::Index<Self> for Outgoing {
    fn index(&self, path: &[usize]) -> anyhow::Result<Self> {
        let mut header = BytesMut::with_capacity(path.len().saturating_add(5));
        let depth = path.len();
        let n = u32::try_from(depth)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
        trace!(n, "encoding path length");
        Leb128Encoder.encode(n, &mut header)?;
        for p in path {
            let p = u32::try_from(*p)
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
            trace!(p, "encoding path element");
            Leb128Encoder.encode(p, &mut header)?;
        }
        match self {
            Self::Opening {
                path: base, conn, ..
            }
            | Self::Active {
                path: base, conn, ..
            } => Ok(Self::Opening {
                header: header.freeze(),
                path: Arc::from([base, path].concat()),
                conn: conn.clone(),
            }),
        }
    }
}

impl AsyncWrite for Outgoing {
    #[instrument(level = "trace", skip_all, fields(?buf))]
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.as_mut().project() {
            OutgoingProj::Opening { path, conn, header } => {
                trace!(?path, "opening connection");
                let tx = ready!(pin!(conn.open_uni()).poll(cx)).map_err(std::io::Error::from)?;
                *self = Self::Active {
                    header: header.clone(),
                    path: Arc::clone(path),
                    conn: conn.clone(),
                    tx,
                };
                self.poll_write(cx, buf)
            }
            OutgoingProj::Active {
                mut tx,
                header,
                path,
                ..
            } => {
                while !header.is_empty() {
                    trace!(?header, "writing header");
                    let n = ready!(AsyncWrite::poll_write(tx.as_mut(), cx, header))?;
                    if n < header.len() {
                        header.advance(n);
                    } else {
                        *header = Bytes::default();
                    }
                }
                trace!(?path, "writing buffer");
                AsyncWrite::poll_write(tx, cx, buf)
            }
        }
    }

    #[instrument(level = "trace", skip_all)]
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.as_mut().project() {
            OutgoingProj::Opening { .. } => Poll::Ready(Ok(())),
            OutgoingProj::Active { tx, .. } => {
                trace!("flushing stream");
                tx.poll_flush(cx)
            }
        }
    }

    #[instrument(level = "trace", skip_all)]
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.as_mut().project() {
            OutgoingProj::Opening { .. } => Poll::Ready(Ok(())),
            OutgoingProj::Active { tx, .. } => {
                trace!("shutting down stream");
                tx.poll_shutdown(cx)
            }
        }
    }
}

pub struct Session {
    conn: Connection,
    incoming: RecvStream,
    outgoing: SendStream,
    reader: JoinHandle<std::io::Result<()>>,
}

impl Session {
    pub fn new(
        conn: Connection,
        incoming: RecvStream,
        outgoing: SendStream,
        index: Arc<std::sync::Mutex<IndexTree>>,
    ) -> Self {
        Self {
            conn: conn.clone(),
            incoming,
            outgoing,
            reader: spawn(demux_connection(index, conn)),
        }
    }
}

impl wrpc_transport::Session for Session {
    #[instrument(level = "trace", skip_all)]
    async fn finish(mut self, res: Result<(), &str>) -> anyhow::Result<Result<(), String>> {
        if let Err(err) = res {
            let mut buf = BytesMut::with_capacity(6 + err.len());
            buf.put_u8(0x01);
            let mut err_buf = buf.split_off(1);
            if let Err(err) = CoreNameEncoder.encode(err, &mut err_buf) {
                warn!(?err, "failed to encode error");
                err_buf.clear();
                if let Err(err) = CoreNameEncoder.encode(err.to_string(), &mut err_buf) {
                    warn!(?err, "failed to encode encoding error");
                    err_buf.clear();
                }
            }
            buf.unsplit(err_buf);
            trace!(?buf, "writing error string");
            self.outgoing
                .write_all(&buf)
                .await
                .context("failed to write `result::error`")?;
        } else {
            self.outgoing
                .write_u8(0x00)
                .await
                .context("failed to write `result::ok`")?;
        }
        let ok = self
            .incoming
            .read_result_status()
            .await
            .context("failed to read result status")?;
        let res = if ok {
            Ok(())
        } else {
            let mut err = String::new();
            self.incoming
                .read_core_name(&mut err)
                .await
                .context("failed to read `result::error` string value")?;
            Err(err)
        };
        self.outgoing
            .finish()
            .context("failed to finish outgoing result stream")?;
        self.incoming
            .stop(VarInt::default())
            .context("failed to stop incoming result stream")?;
        let (outgoing, incoming) = try_join!(
            async {
                self.outgoing
                    .stopped()
                    .await
                    .context("outgoing stream processing failed")
            },
            async {
                self.incoming
                    .received_reset()
                    .await
                    .context("incoming stream processing failed")
            }
        )?;
        if let Some(code) = outgoing {
            if code != VarInt::default() {
                bail!("outgoing stream processing failed with code: {code}")
            }
        }
        if let Some(code) = incoming {
            if code != VarInt::default() {
                bail!("incoming stream processing failed with code: {code}")
            }
        }
        self.conn.close(VarInt::default(), &[]);
        let err = self.conn.closed().await;
        match err {
            ConnectionError::ApplicationClosed(ref e) => {
                if e.error_code != VarInt::default() {
                    bail!(err)
                }
            }
            ConnectionError::LocallyClosed => {}
            ConnectionError::VersionMismatch
            | ConnectionError::TransportError(_)
            | ConnectionError::ConnectionClosed(_)
            | ConnectionError::Reset
            | ConnectionError::TimedOut
            | ConnectionError::CidsExhausted => return Err(err.into()),
        }
        self.reader
            .await
            .context("failed to stop frame reader")?
            .context("frame reader failed")?;
        Ok(res)
    }
}

async fn demux_connection(
    index: Arc<std::sync::Mutex<IndexTree>>,
    conn: Connection,
) -> std::io::Result<()> {
    loop {
        // TODO
        trace!("accepting async stream");
        let mut rx = match conn.accept_uni().await {
            Ok(rx) => rx,
            Err(err) => match err {
                ConnectionError::ApplicationClosed(ref e) => {
                    if e.error_code != VarInt::default() {
                        return Err(err.into());
                    }
                    return Ok(());
                }
                ConnectionError::LocallyClosed => return Ok(()),
                ConnectionError::VersionMismatch
                | ConnectionError::TransportError(_)
                | ConnectionError::ConnectionClosed(_)
                | ConnectionError::Reset
                | ConnectionError::TimedOut
                | ConnectionError::CidsExhausted => return Err(err.into()),
            },
        };
        // TODO: Use the `CoreVecDecoder`
        trace!("reading path length");
        let n = rx.read_u32_leb128().await?;
        let n = n
            .try_into()
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
        trace!(n, "read path length");
        let mut path = Vec::with_capacity(n);
        for i in 0..n {
            trace!(i, "reading path element");
            let p = rx.read_u32_leb128().await?;
            let p = p
                .try_into()
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
            path.push(p);
        }
        trace!("locking path index tree");
        let mut lock = index
            .lock()
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err.to_string()))?;
        let tx = lock.take_tx(&path).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("`{path:?}` subscription not found"),
            )
        })?;
        trace!(?path, "sending stream to receiver");
        tx.send(rx).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "stream receiver closed")
        })?;
    }
}

impl wrpc_transport::Invoke for Client {
    type Context = ();
    type Session = Session;
    type Outgoing = Outgoing;
    type Incoming = Incoming;

    #[instrument(level = "trace", skip(self))]
    async fn invoke(
        &self,
        cx: Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: &[&[Option<usize>]],
    ) -> anyhow::Result<Invocation> {
        let san = san(instance, func);
        trace!(?san, "establishing connection");
        let conn = self
            .endpoint
            .connect(self.addr, &san)
            .context("failed to connect to endpoint")?;
        let conn = conn.await.context("failed to establish connection")?;

        let ((mut param_tx, ret_rx), res_rx, res_tx) = try_join!(
            async {
                trace!("opening parameter stream");
                conn.open_bi()
                    .await
                    .context("failed to open parameter stream")
            },
            async {
                trace!("accepting server result stream");
                let mut rx = conn
                    .accept_uni()
                    .await
                    .context("failed to accept server result stream")?;
                trace!("reading server result stream header");
                let x = rx
                    .read_u8()
                    .await
                    .context("failed to read server result stream header")?;
                ensure!(x == PROTOCOL);
                Ok(rx)
            },
            async {
                trace!("opening client result stream");
                let mut tx = conn
                    .open_uni()
                    .await
                    .context("failed to open client result stream")?;
                debug!("writing client result stream header");
                tx.write_u8(PROTOCOL)
                    .await
                    .context("failed to write client result stream header")?;
                Ok(tx)
            },
        )?;
        res_tx
            .set_priority(1)
            .context("failed to set result stream priority")?;
        let index = Arc::new(std::sync::Mutex::new(paths.iter().collect()));
        trace!(?params, "writing parameters");
        param_tx
            .write_all_chunks(&mut [Bytes::from_static(&[PROTOCOL]), params])
            .await
            .context("failed to write parameters")?;
        Ok(Invocation {
            outgoing: Outgoing::Active {
                header: Bytes::default(),
                path: Arc::from([]),
                conn: conn.clone(),
                tx: param_tx,
            },
            incoming: Incoming::Active {
                index: Arc::clone(&index),
                path: Arc::from([]),
                rx: ret_rx,
            },
            session: Session::new(conn, res_rx, res_tx, index),
        })
    }
}

#[instrument(level = "trace", skip_all)]
async fn serve_connection(
    conn: Connection,
    paths: &[&[Option<usize>]],
) -> anyhow::Result<Invocation> {
    let ((ret_tx, param_rx), res_rx, res_tx) = try_join!(
        async {
            trace!("accepting parameter stream");
            let (tx, mut rx) = conn
                .accept_bi()
                .await
                .context("failed to accept parameter stream")?;
            trace!("reading parameter stream header");
            let x = rx
                .read_u8()
                .await
                .context("failed to read parameter stream header")?;
            ensure!(x == PROTOCOL);
            Ok((tx, rx))
        },
        async {
            trace!("accepting client result stream");
            let mut rx = conn
                .accept_uni()
                .await
                .context("failed to accept client result stream")?;
            trace!("reading client result stream header");
            let x = rx
                .read_u8()
                .await
                .context("failed to read client result stream header")?;
            ensure!(x == PROTOCOL);
            Ok(rx)
        },
        async {
            trace!("opening server result stream");
            let mut tx = conn
                .open_uni()
                .await
                .context("failed to open server result stream")?;
            debug!("writing server result stream header");
            tx.write_u8(PROTOCOL)
                .await
                .context("failed to write server result stream header")?;
            Ok(tx)
        }
    )?;
    res_tx
        .set_priority(1)
        .context("failed to set result stream priority")?;
    debug!("writing server result stream header");
    let index = Arc::new(std::sync::Mutex::new(paths.iter().collect()));
    Ok(Invocation {
        outgoing: Outgoing::Active {
            header: Bytes::default(),
            path: Arc::from([]),
            conn: conn.clone(),
            tx: ret_tx,
        },
        incoming: Incoming::Active {
            index: Arc::clone(&index),
            path: Arc::from([]),
            rx: param_rx,
        },
        session: Session::new(conn, res_rx, res_tx, index),
    })
}

impl wrpc_transport::Serve for Server {
    type Context = ();
    type Session = Session;
    type Outgoing = Outgoing;
    type Incoming = Incoming;

    #[instrument(level = "trace", skip(self))]
    async fn serve(
        &self,
        instance: &str,
        func: &str,
        paths: &[&[Option<usize>]],
    ) -> anyhow::Result<impl Stream<Item = anyhow::Result<(Self::Context, Invocation)>>> {
        let san = san(instance, func);
        let (tx, rx) = mpsc::channel(1024);
        let mut handlers = self.0.lock().await;
        match handlers.entry(san) {
            hash_map::Entry::Occupied(_) => {
                bail!("handler for `{func}` from `{instance}` already exists")
            }
            hash_map::Entry::Vacant(entry) => {
                entry.insert(tx);
            }
        }
        let span = tracing::Span::current();
        Ok(ReceiverStream::new(rx).then(move |conn| {
            async move {
                let invocation = serve_connection(conn, paths).await?;
                Ok(((), invocation))
            }
            .instrument(span.clone())
        }))
    }
}
