//! wRPC TCP transport using `wasi:sockets`

use core::pin::Pin;
use core::task::{ready, Context, Poll, Waker};

use std::sync::Arc;

use anyhow::{bail, Context as _};
use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;
use tracing::{debug, instrument, trace};
use wasi::io::poll::Pollable;
use wasi::io::streams::{InputStream, OutputStream, StreamError};
use wasi::sockets::instance_network::instance_network;
use wasi::sockets::ip_name_lookup::resolve_addresses;
use wasi::sockets::network::{
    IpAddress, IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, Ipv6SocketAddress, Network,
};
use wasi::sockets::tcp::ShutdownType;
use wasi::sockets::tcp_create_socket::{create_tcp_socket, TcpSocket};

use crate::frame::{Incoming, Outgoing};
use crate::Invoke;

/// [Invoke] implementation of a TCP transport using `wasi:sockets`
#[derive(Clone, Copy, Debug)]
pub struct Client<T, N = Network> {
    addr: T,
    network: N,
}

impl<T, N> Client<T, N> {
    /// Constructs a new [Client]
    pub fn new(network: N, addr: T) -> Self {
        Self { addr, network }
    }
}

impl From<String> for Client<Box<str>, Network> {
    fn from(addr: String) -> Self {
        Self {
            addr: addr.into_boxed_str(),
            network: instance_network(),
        }
    }
}

impl From<Box<str>> for Client<Box<str>, Network> {
    fn from(addr: Box<str>) -> Self {
        Self {
            addr,
            network: instance_network(),
        }
    }
}

impl<'a> From<&'a str> for Client<&'a str, Network> {
    fn from(addr: &'a str) -> Self {
        Self {
            addr,
            network: instance_network(),
        }
    }
}

impl From<IpSocketAddress> for Client<IpSocketAddress, Network> {
    fn from(addr: IpSocketAddress) -> Self {
        Self {
            addr,
            network: instance_network(),
        }
    }
}

struct IncomingStream {
    rx: InputStream,
    socket: Arc<TcpSocket>,
    pollables: mpsc::UnboundedSender<(Pollable, Waker)>,
}

impl AsyncRead for IncomingStream {
    #[instrument(level = "trace", skip_all, ret(level = "trace"))]
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if buf.capacity() == 0 {
            return Poll::Ready(Ok(()));
        }
        let len = buf.capacity().try_into().unwrap_or(u64::MAX);
        let chunk = match self.rx.read(len) {
            Ok(chunk) => chunk,
            Err(StreamError::Closed) => return Poll::Ready(Ok(())),
            Err(StreamError::LastOperationFailed(err)) => {
                return Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::Other, err)))
            }
        };
        if chunk.is_empty() {
            trace!("sending read pollable");
            self.pollables
                .send((self.rx.subscribe(), cx.waker().clone()))
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))?;
            return Poll::Pending;
        }
        buf.put_slice(&chunk);
        Poll::Ready(Ok(()))
    }
}

impl Drop for IncomingStream {
    #[instrument(level = "trace", skip_all, ret(level = "trace"))]
    fn drop(&mut self) {
        trace!("shutting down receive end of the stream");
        if let Err(err) = self.socket.shutdown(ShutdownType::Receive) {
            debug!(?err, "failed to shut down receive end of the stream")
        }
    }
}

struct OutgoingStream {
    tx: OutputStream,
    socket: Arc<TcpSocket>,
    pollables: mpsc::UnboundedSender<(Pollable, Waker)>,
}

impl AsyncWrite for OutgoingStream {
    #[instrument(level = "trace", skip_all, ret(level = "trace"))]
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        trace!("checking write budget");
        let n = self
            .tx
            .check_write()
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))?;
        if n == 0 {
            trace!("sending write pollable");
            self.pollables
                .send((self.tx.subscribe(), cx.waker().clone()))
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))?;
            return Poll::Pending;
        }
        let max = buf.len();
        let n = usize::try_from(n).map(|n| n.min(max)).unwrap_or(max);
        let buf = &buf[..n];
        trace!(?buf, "writing buffer");
        self.tx
            .write(buf)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))?;
        Poll::Ready(Ok(n))
    }

    #[instrument(level = "trace", skip_all, ret(level = "trace"))]
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.tx
            .flush()
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))?;
        Poll::Ready(Ok(()))
    }

    #[instrument(level = "trace", skip_all, ret(level = "trace"))]
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        ready!(self.as_mut().poll_flush(cx))?;
        trace!("shutting down send end of the stream");
        self.socket
            .shutdown(ShutdownType::Send)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))?;
        Poll::Ready(Ok(()))
    }
}

#[instrument(level = "trace", skip(socket, paths, params, pollables), fields(params = format!("{params:02x?}")))]
async fn finish_invoke<P>(
    socket: TcpSocket,
    instance: &str,
    func: &str,
    params: Bytes,
    paths: impl AsRef<[P]> + Send,
    pollables: mpsc::UnboundedSender<(Pollable, Waker)>,
) -> anyhow::Result<(Outgoing, Incoming)>
where
    P: AsRef<[Option<usize>]> + Send + Sync,
{
    socket.subscribe().block();
    let (rx, tx) = socket
        .finish_connect()
        .context("failed to finish connection")?;
    let socket = Arc::new(socket);
    crate::frame::invoke(
        OutgoingStream {
            tx,
            socket: Arc::clone(&socket),
            pollables: pollables.clone(),
        },
        IncomingStream {
            rx,
            socket,
            pollables,
        },
        instance,
        func,
        params,
        paths,
    )
    .await
}

#[instrument(level = "trace", skip_all, fields(params = format!("{params:02x?}")))]
async fn invoke_ip<P>(
    network: &Network,
    addr: IpSocketAddress,
    instance: &str,
    func: &str,
    params: Bytes,
    paths: impl AsRef<[P]> + Send,
    pollables: mpsc::UnboundedSender<(Pollable, Waker)>,
) -> anyhow::Result<(Outgoing, Incoming)>
where
    P: AsRef<[Option<usize>]> + Send + Sync,
{
    let socket = match addr {
        IpSocketAddress::Ipv4(..) => create_tcp_socket(IpAddressFamily::Ipv4),
        IpSocketAddress::Ipv6(..) => create_tcp_socket(IpAddressFamily::Ipv6),
    }
    .context("failed to create TCP socket")?;
    socket
        .start_connect(network, addr)
        .context("failed to start connection")?;
    finish_invoke(socket, instance, func, params, paths, pollables).await
}

#[instrument(level = "trace", skip_all, fields(params = format!("{params:02x?}")))]
async fn invoke_unresolved<P>(
    network: &Network,
    addr: &str,
    instance: &str,
    func: &str,
    params: Bytes,
    paths: impl AsRef<[P]> + Send,
    pollables: mpsc::UnboundedSender<(Pollable, Waker)>,
) -> anyhow::Result<(Outgoing, Incoming)>
where
    P: AsRef<[Option<usize>]> + Send + Sync,
{
    let (addr, port) = addr.rsplit_once(":").context("address is not valid")?;
    let port = port.parse().context("port is not valid")?;
    let addrs =
        resolve_addresses(network, addr).with_context(|| format!("failed to resolve `{addr}`"))?;
    addrs.subscribe().block();
    let Some(addr) = addrs
        .resolve_next_address()
        .with_context(|| format!("failed to resolve `{addr}` address"))?
    else {
        bail!("`{addr}` cannot be resolved")
    };
    let (socket, addr) = match addr {
        IpAddress::Ipv4(address) => {
            let socket =
                create_tcp_socket(IpAddressFamily::Ipv4).context("failed to create TCP socket")?;
            (
                socket,
                IpSocketAddress::Ipv4(Ipv4SocketAddress { port, address }),
            )
        }
        IpAddress::Ipv6(address) => {
            let socket =
                create_tcp_socket(IpAddressFamily::Ipv6).context("failed to create TCP socket")?;
            (
                socket,
                IpSocketAddress::Ipv6(Ipv6SocketAddress {
                    port,
                    flow_info: 0,
                    address,
                    scope_id: 0,
                }),
            )
        }
    };
    socket
        .start_connect(network, addr)
        .context("failed to start connection")?;
    finish_invoke(socket, instance, func, params, paths, pollables).await
}

impl Invoke for Client<&str, &Network> {
    type Context = mpsc::UnboundedSender<(Pollable, Waker)>;
    type Outgoing = Outgoing;
    type Incoming = Incoming;

    async fn invoke<P>(
        &self,
        pollables: Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Self::Outgoing, Self::Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        invoke_unresolved(
            self.network,
            self.addr,
            instance,
            func,
            params,
            paths,
            pollables,
        )
        .await
    }
}

impl Invoke for Client<&str, Network> {
    type Context = mpsc::UnboundedSender<(Pollable, Waker)>;
    type Outgoing = Outgoing;
    type Incoming = Incoming;

    async fn invoke<P>(
        &self,
        pollables: Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Self::Outgoing, Self::Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        invoke_unresolved(
            &self.network,
            self.addr,
            instance,
            func,
            params,
            paths,
            pollables,
        )
        .await
    }
}

impl Invoke for Client<Box<str>, Network> {
    type Context = mpsc::UnboundedSender<(Pollable, Waker)>;
    type Outgoing = Outgoing;
    type Incoming = Incoming;

    async fn invoke<P>(
        &self,
        pollables: Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Self::Outgoing, Self::Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        invoke_unresolved(
            &self.network,
            &self.addr,
            instance,
            func,
            params,
            paths,
            pollables,
        )
        .await
    }
}

impl Invoke for Client<IpSocketAddress, &Network> {
    type Context = mpsc::UnboundedSender<(Pollable, Waker)>;
    type Outgoing = Outgoing;
    type Incoming = Incoming;

    async fn invoke<P>(
        &self,
        pollables: Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Self::Outgoing, Self::Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        invoke_ip(
            self.network,
            self.addr,
            instance,
            func,
            params,
            paths,
            pollables,
        )
        .await
    }
}

impl Invoke for Client<IpSocketAddress, Network> {
    type Context = mpsc::UnboundedSender<(Pollable, Waker)>;
    type Outgoing = Outgoing;
    type Incoming = Incoming;

    async fn invoke<P>(
        &self,
        pollables: Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Self::Outgoing, Self::Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        invoke_ip(
            &self.network,
            self.addr,
            instance,
            func,
            params,
            paths,
            pollables,
        )
        .await
    }
}

// TODO: Implement `Accept`
