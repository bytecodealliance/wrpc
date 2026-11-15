//! wRPC TCP transport using `wasi:sockets`

use core::pin::Pin;
use core::task::{Context, Poll, Waker, ready};

use std::sync::Arc;

use anyhow::{Context as _, bail};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;
use tracing::{debug, instrument, trace};
use wasi::io::poll::Pollable;
use wasi::io::streams::{InputStream, OutputStream, StreamError};
use wasi::sockets::ip_name_lookup::resolve_addresses;
use wasi::sockets::network::{
    IpAddress, IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, Ipv6SocketAddress, Network,
};
use wasi::sockets::tcp::ShutdownType;
use wasi::sockets::tcp_create_socket::{TcpSocket, create_tcp_socket};

/// Receive end of a TCP connection established by [connect]
pub struct IncomingStream {
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
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let len = buf.remaining().try_into().unwrap_or(u64::MAX);
        let chunk = match self.rx.read(len) {
            Ok(chunk) => chunk,
            Err(StreamError::Closed) => return Poll::Ready(Ok(())),
            Err(StreamError::LastOperationFailed(err)) => {
                return Poll::Ready(Err(std::io::Error::other(err)));
            }
        };
        if chunk.is_empty() {
            trace!("sending read pollable");
            self.pollables
                .send((self.rx.subscribe(), cx.waker().clone()))
                .map_err(std::io::Error::other)?;
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

/// Send end of a TCP connection established by [connect]
pub struct OutgoingStream {
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
        let n = self.tx.check_write().map_err(std::io::Error::other)?;
        if n == 0 {
            trace!("sending write pollable");
            self.pollables
                .send((self.tx.subscribe(), cx.waker().clone()))
                .map_err(std::io::Error::other)?;
            return Poll::Pending;
        }
        let max = buf.len();
        let n = usize::try_from(n).map(|n| n.min(max)).unwrap_or(max);
        let buf = &buf[..n];
        trace!(?buf, "writing buffer");
        self.tx.write(buf).map_err(std::io::Error::other)?;
        Poll::Ready(Ok(n))
    }

    #[instrument(level = "trace", skip_all, ret(level = "trace"))]
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.tx.flush().map_err(std::io::Error::other)?;
        let n = self.tx.check_write().map_err(std::io::Error::other)?;
        if n == 0 {
            trace!("sending flush pollable");
            self.pollables
                .send((self.tx.subscribe(), cx.waker().clone()))
                .map_err(std::io::Error::other)?;
            return Poll::Pending;
        }
        Poll::Ready(Ok(()))
    }

    #[instrument(level = "trace", skip_all, ret(level = "trace"))]
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        ready!(self.as_mut().poll_flush(cx))?;
        trace!("shutting down send end of the stream");
        self.socket
            .shutdown(ShutdownType::Send)
            .map_err(std::io::Error::other)?;
        Poll::Ready(Ok(()))
    }
}

/// Connects to `addr`, sending the pollables of pending I/O along with the wakers to wake once
/// they are ready on `pollables`
#[instrument(level = "trace", skip_all)]
pub fn connect_ip(
    network: &Network,
    addr: IpSocketAddress,
    pollables: mpsc::UnboundedSender<(Pollable, Waker)>,
) -> anyhow::Result<(OutgoingStream, IncomingStream)> {
    let socket = match addr {
        IpSocketAddress::Ipv4(..) => create_tcp_socket(IpAddressFamily::Ipv4),
        IpSocketAddress::Ipv6(..) => create_tcp_socket(IpAddressFamily::Ipv6),
    }
    .context("failed to create TCP socket")?;
    socket
        .start_connect(network, addr)
        .context("failed to start connection")?;
    socket.subscribe().block();
    let (rx, tx) = socket
        .finish_connect()
        .context("failed to finish connection")?;
    let socket = Arc::new(socket);
    let tx = OutgoingStream {
        tx,
        socket: Arc::clone(&socket),
        pollables: pollables.clone(),
    };
    let rx = IncomingStream {
        rx,
        socket,
        pollables,
    };
    Ok((tx, rx))
}

/// Resolves `addr` in `host:port` form and connects to it, see [`connect_ip`]
#[instrument(level = "trace", skip_all)]
pub fn connect(
    network: &Network,
    addr: &str,
    pollables: mpsc::UnboundedSender<(Pollable, Waker)>,
) -> anyhow::Result<(OutgoingStream, IncomingStream)> {
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
    let addr = match addr {
        IpAddress::Ipv4(address) => IpSocketAddress::Ipv4(Ipv4SocketAddress { port, address }),
        IpAddress::Ipv6(address) => IpSocketAddress::Ipv6(Ipv6SocketAddress {
            port,
            flow_info: 0,
            address,
            scope_id: 0,
        }),
    };
    connect_ip(network, addr, pollables)
}
