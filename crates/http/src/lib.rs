//! wRPC HTTP transport

use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use core::task::{Context, Poll, ready};

use std::sync::Arc;

use anyhow::ensure;
use bytes::{Bytes, BytesMut};
use http_body::Frame;
use http_body_util::BodyExt as _;
use http_body_util::combinators::MapErr;
use pin_project_lite::pin_project;
use tokio::io::{ReadHalf, SimplexStream, WriteHalf, simplex};
use tokio_util::io::{StreamReader, poll_read_buf};

/// Body buffer size
pub const DEFAULT_BODY_BUFFER_SIZE: usize = 8192;

pub type IncomingBodyDataStream<T> =
    http_body_util::BodyDataStream<MapErr<hyper::body::Incoming, T>>;

pub type IncomingBodyDataReader<T> = StreamReader<IncomingBodyDataStream<T>, Bytes>;

pub type DynIncomingBodyDataReader =
    IncomingBodyDataReader<Box<dyn FnMut(hyper::Error) -> std::io::Error + Send + Sync + 'static>>;

pub type OutgoingBodyDataWriter = WriteHalf<SimplexStream>;

pin_project! {
    #[project = OutgoingBodyProj]
    #[derive(Debug)]
    pub struct OutgoingBody {
        buffer: BytesMut,
        #[pin]
        stream: ReadHalf<SimplexStream>,
    }
}

impl OutgoingBody {
    pub fn new(buffer: BytesMut) -> (Self, OutgoingBodyDataWriter) {
        let (rx, tx) = simplex(buffer.len().max(DEFAULT_BODY_BUFFER_SIZE));
        (Self { buffer, stream: rx }, tx)
    }
}

impl http_body::Body for OutgoingBody {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if !self.buffer.is_empty() {
            return Poll::Ready(Some(Ok(Frame::data(self.buffer.split().freeze()))));
        }

        let this = self.as_mut().project();
        ready!(poll_read_buf(this.stream, cx, this.buffer))?;
        let buf = self.buffer.split().freeze();
        if buf.is_empty() {
            Poll::Ready(None)
        } else {
            Poll::Ready(Some(Ok(Frame::data(buf))))
        }
    }
}

pub fn new_request(
    mut parts: http::request::Parts,
    buffer: BytesMut,
) -> (http::Request<OutgoingBody>, OutgoingBodyDataWriter) {
    let (rx, tx) = OutgoingBody::new(buffer);
    parts.method = http::Method::POST;
    (http::Request::from_parts(parts, rx), tx)
}

pub fn io_error_from_hyper(err: hyper::Error) -> std::io::Error {
    if err.is_timeout() {
        std::io::Error::new(std::io::ErrorKind::TimedOut, err)
    } else {
        std::io::Error::other(err)
    }
}

pub fn data_stream_from_incoming(
    body: hyper::body::Incoming,
) -> IncomingBodyDataStream<impl FnMut(hyper::Error) -> std::io::Error> {
    body.map_err(io_error_from_hyper).into_data_stream()
}

pub fn data_reader_from_incoming(
    body: hyper::body::Incoming,
) -> IncomingBodyDataReader<impl FnMut(hyper::Error) -> std::io::Error> {
    StreamReader::new(data_stream_from_incoming(body))
}

pub fn handle_response(
    res: http::Response<hyper::body::Incoming>,
) -> anyhow::Result<IncomingBodyDataReader<impl FnMut(hyper::Error) -> std::io::Error>> {
    let (http::response::Parts { status, .. }, rx) = res.into_parts();
    ensure!(status.is_success(), "HTTP request failed");
    Ok(data_reader_from_incoming(rx))
}

/// wRPC Server
#[derive(Debug, Default)]
#[repr(transparent)]
pub struct Server(
    pub  Arc<
        wrpc_transport::frame::Server<
            http::request::Parts,
            DynIncomingBodyDataReader,
            OutgoingBodyDataWriter,
        >,
    >,
);

impl Deref for Server {
    type Target = Arc<
        wrpc_transport::frame::Server<
            http::request::Parts,
            DynIncomingBodyDataReader,
            OutgoingBodyDataWriter,
        >,
    >;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Server {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T> From<T> for Server
where
    T: Into<
        Arc<
            wrpc_transport::frame::Server<
                http::request::Parts,
                DynIncomingBodyDataReader,
                OutgoingBodyDataWriter,
            >,
        >,
    >,
{
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl Server {
    /// Construct a new [Server]
    pub fn new() -> Self {
        Self::default()
    }
}

impl hyper::service::Service<http::Request<hyper::body::Incoming>> for Server {
    type Response = http::Response<OutgoingBody>;
    type Error = wrpc_transport::frame::AcceptError<
        http::request::Parts,
        DynIncomingBodyDataReader,
        OutgoingBodyDataWriter,
    >;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, req: http::Request<hyper::body::Incoming>) -> Self::Future {
        let (parts, body) = req.into_parts();
        let map_err: Box<dyn FnMut(hyper::Error) -> std::io::Error + Send + Sync + 'static> =
            Box::new(io_error_from_hyper);
        let body = body.map_err(map_err).into_data_stream();
        let body = StreamReader::new(body);
        let (rx, tx) = OutgoingBody::new(BytesMut::default());
        let srv = Arc::clone(&self.0);
        Box::pin(async move {
            srv.accept(parts, tx, body).await?;
            Ok(http::Response::new(rx))
        })
    }
}

/// Client wrapper
#[cfg(feature = "client")]
#[derive(Debug, Default)]
#[repr(transparent)]
pub struct Client<T>(pub T);

#[cfg(feature = "client")]
impl<T> Deref for Client<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(feature = "client")]
impl<T> DerefMut for Client<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[cfg(feature = "client")]
impl<T> From<T> for Client<T> {
    fn from(c: T) -> Self {
        Self(c)
    }
}

#[cfg(feature = "client")]
impl<T> Client<T> {
    pub fn new(c: T) -> Self {
        Self(c)
    }
}

#[cfg(all(feature = "client", feature = "http2"))]
impl wrpc_transport::Invoke for Client<hyper::client::conn::http2::SendRequest<OutgoingBody>> {
    type Context = http::request::Parts;

    #[tracing::instrument(level = "trace", skip(self, cx, paths, params), fields(params = format!("{params:02x?}")))]
    async fn invoke<P>(
        &self,
        cx: Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(
        wrpc_transport::frame::Outgoing,
        wrpc_transport::frame::Incoming,
    )>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        use anyhow::Context as _;

        let mut sender = self.0.clone();
        sender.ready().await.context("HTTP connection closed")?;

        let mut buf = BytesMut::with_capacity(DEFAULT_BODY_BUFFER_SIZE);
        wrpc_transport::frame::encode_invocation(&mut buf, instance, func, &params)
            .context("failed to encode invocation")?;
        let (req, tx) = new_request(cx, buf);

        let res = sender
            .send_request(req)
            .await
            .context("failed to send HTTP request")?;
        let rx = handle_response(res)?;

        Ok((
            wrpc_transport::frame::Outgoing::new(tx, |_, _| async {}),
            wrpc_transport::frame::Incoming::new(rx, paths.as_ref(), |_, _| async {}),
        ))
    }
}

#[cfg(feature = "client-legacy")]
impl<C> wrpc_transport::Invoke for Client<hyper_util::client::legacy::Client<C, OutgoingBody>>
where
    C: hyper_util::client::legacy::connect::Connect + Clone + Send + Sync + 'static,
{
    type Context = http::request::Parts;

    #[tracing::instrument(level = "trace", skip(self, cx, paths, params), fields(params = format!("{params:02x?}")))]
    async fn invoke<P>(
        &self,
        cx: Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(
        wrpc_transport::frame::Outgoing,
        wrpc_transport::frame::Incoming,
    )>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        use anyhow::Context as _;

        let mut buf = BytesMut::with_capacity(DEFAULT_BODY_BUFFER_SIZE);
        wrpc_transport::frame::encode_invocation(&mut buf, instance, func, &params)
            .context("failed to encode invocation")?;
        let (req, tx) = new_request(cx, buf);

        let res = self
            .0
            .request(req)
            .await
            .context("failed to send HTTP request")?;
        let rx = handle_response(res)?;

        Ok((
            wrpc_transport::frame::Outgoing::new(tx, |_, _| async {}),
            wrpc_transport::frame::Incoming::new(rx, paths.as_ref(), |_, _| async {}),
        ))
    }
}
