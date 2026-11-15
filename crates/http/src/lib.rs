//! wRPC HTTP transport

use core::pin::Pin;
use core::task::{Context, Poll, ready};

use bytes::{Bytes, BytesMut};
use futures::stream::BoxStream;
use futures::{Stream, StreamExt as _, TryStreamExt as _, stream};
use http_body::Frame;
use http_body_util::combinators::MapErr;
use http_body_util::{BodyDataStream, BodyExt as _};
use pin_project_lite::pin_project;
use tokio::io::{AsyncReadExt as _, DuplexStream, duplex};
use tokio::sync::mpsc;
use tokio_util::io::{StreamReader, poll_read_buf};

/// Body buffer size
pub const DEFAULT_BODY_BUFFER_SIZE: usize = 8192;

/// Reader of the data of a request body, see [Server]
pub type IncomingBodyDataReader = StreamReader<
    BodyDataStream<MapErr<hyper::body::Incoming, fn(hyper::Error) -> std::io::Error>>,
    Bytes,
>;

/// Reader of the data of a response body, see [`data_reader_from_response`]
pub type ResponseBodyReader = StreamReader<BoxStream<'static, std::io::Result<Bytes>>, Bytes>;

pin_project! {
    #[project = OutgoingBodyProj]
    #[derive(Debug)]
    pub struct OutgoingBody {
        buffer: BytesMut,
        #[pin]
        stream: DuplexStream,
    }
}

impl OutgoingBody {
    #[must_use]
    pub fn new() -> (Self, DuplexStream) {
        let (rx, tx) = duplex(DEFAULT_BODY_BUFFER_SIZE);
        (
            Self {
                buffer: BytesMut::default(),
                stream: rx,
            },
            tx,
        )
    }
}

impl http_body::Body for OutgoingBody {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.as_mut().project();
        this.buffer.reserve(DEFAULT_BODY_BUFFER_SIZE);
        ready!(poll_read_buf(this.stream, cx, this.buffer))?;
        let buf = self.buffer.split().freeze();
        if buf.is_empty() {
            Poll::Ready(None)
        } else {
            Poll::Ready(Some(Ok(Frame::data(buf))))
        }
    }
}

pub fn new_request(mut parts: http::request::Parts) -> (http::Request<OutgoingBody>, DuplexStream) {
    let (rx, tx) = OutgoingBody::new();
    parts.method = http::Method::POST;
    (http::Request::from_parts(parts, rx), tx)
}

#[must_use]
pub fn io_error_from_hyper(err: hyper::Error) -> std::io::Error {
    if err.is_timeout() {
        std::io::Error::new(std::io::ErrorKind::TimedOut, err)
    } else {
        std::io::Error::other(err)
    }
}

pub fn data_stream_from_incoming(
    body: hyper::body::Incoming,
) -> BodyDataStream<MapErr<hyper::body::Incoming, impl FnMut(hyper::Error) -> std::io::Error>> {
    body.map_err(io_error_from_hyper).into_data_stream()
}

#[must_use]
pub fn data_reader_from_incoming(
    body: hyper::body::Incoming,
) -> StreamReader<
    BodyDataStream<MapErr<hyper::body::Incoming, impl FnMut(hyper::Error) -> std::io::Error>>,
    Bytes,
> {
    StreamReader::new(data_stream_from_incoming(body))
}

/// Returns a reader of the body of the response `res` resolves to.
///
/// The response is awaited lazily, on first read, so that the request body
/// can be completed independently of the response.
/// Response status codes other than 2xx surface as an [`std::io::Error`] on read, carrying the
/// status and the first 4 KiB of the response body.
pub fn data_reader_from_response<E>(
    res: impl Future<Output = Result<http::Response<hyper::body::Incoming>, E>> + Send + 'static,
) -> ResponseBodyReader
where
    E: std::error::Error + Send + Sync + 'static,
{
    let body = stream::once(Box::pin(async move {
        let res = res.await.map_err(std::io::Error::other)?;
        let (http::response::Parts { status, .. }, body) = res.into_parts();
        if !status.is_success() {
            let mut buf = Vec::default();
            let _ = data_reader_from_incoming(body)
                .take(4096)
                .read_to_end(&mut buf)
                .await;
            return Err(std::io::Error::other(if buf.is_empty() {
                format!("HTTP request failed with status `{status}`")
            } else {
                format!(
                    "HTTP request failed with status `{status}`: {}",
                    String::from_utf8_lossy(&buf)
                )
            }));
        }
        Ok(data_stream_from_incoming(body))
    }))
    .try_flatten();
    StreamReader::new(body.boxed())
}

/// Creates a [Service], which passes each request to the [Server] as an invocation, waiting
/// while `buffer` invocations are not yet accepted
///
/// # Panics
///
/// Panics if `buffer` is zero
#[must_use]
pub fn channel(buffer: usize) -> (Service, Server) {
    let (tx, rx) = mpsc::channel(buffer);
    (Service(tx), Server(rx))
}

/// [`hyper::service::Service`], which passes each request to the [Server] as an invocation,
/// see [channel]
#[derive(Clone, Debug)]
pub struct Service(mpsc::Sender<(http::request::Parts, DuplexStream, IncomingBodyDataReader)>);

impl hyper::service::Service<http::Request<hyper::body::Incoming>> for Service {
    type Response = http::Response<OutgoingBody>;
    type Error = std::io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, req: http::Request<hyper::body::Incoming>) -> Self::Future {
        let (parts, body) = req.into_parts();
        let body = body
            .map_err(io_error_from_hyper as fn(_) -> _)
            .into_data_stream();
        let body = StreamReader::new(body);
        let (rx, tx) = OutgoingBody::new();
        let srv = self.0.clone();
        Box::pin(async move {
            if srv.send((parts, tx, body)).await.is_err() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "server dropped",
                ));
            }
            Ok(http::Response::new(rx))
        })
    }
}

/// wRPC server, which yields the invocations passed to it by the [Service], see [channel].
/// The stream ends once all [Service]s are dropped.
#[derive(Debug)]
pub struct Server(mpsc::Receiver<(http::request::Parts, DuplexStream, IncomingBodyDataReader)>);

impl Stream for Server {
    type Item = Result<
        (http::request::Parts, DuplexStream, IncomingBodyDataReader),
        core::convert::Infallible,
    >;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.0.poll_recv(cx).map(|item| item.map(Ok))
    }
}

/// Sends a wRPC invocation request with `parts` on `sender`, returning the request body writer
/// and the response body reader
#[cfg(all(feature = "client", feature = "http2"))]
#[tracing::instrument(level = "trace", skip_all)]
pub async fn open(
    mut sender: hyper::client::conn::http2::SendRequest<OutgoingBody>,
    parts: http::request::Parts,
) -> Result<(DuplexStream, ResponseBodyReader), hyper::Error> {
    sender.ready().await?;
    let (req, tx) = new_request(parts);
    let rx = data_reader_from_response(sender.send_request(req));
    Ok((tx, rx))
}

/// Sends a wRPC invocation request with `parts` using `client`, returning the request body writer
/// and the response body reader
#[cfg(feature = "client-legacy")]
pub fn open_pooled<C>(
    client: &hyper_util::client::legacy::Client<C, OutgoingBody>,
    parts: http::request::Parts,
) -> (DuplexStream, ResponseBodyReader)
where
    C: hyper_util::client::legacy::connect::Connect + Clone + Send + Sync + 'static,
{
    let (req, tx) = new_request(parts);
    let rx = data_reader_from_response(client.request(req));
    (tx, rx)
}
