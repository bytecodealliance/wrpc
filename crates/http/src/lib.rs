//! wRPC HTTP transport

use core::pin::{Pin, pin};
use core::task::{Context, Poll, ready};

use bytes::{Bytes, BytesMut};
use futures::stream::BoxStream;
use futures::{StreamExt as _, TryStreamExt as _, stream};
use http_body::Frame;
use http_body_util::combinators::MapErr;
use http_body_util::{BodyDataStream, BodyExt as _};
use pin_project_lite::pin_project;
use tokio::io::{AsyncReadExt as _, DuplexStream, duplex};
use tokio_util::io::{StreamReader, poll_read_buf};

/// Body buffer size
pub const DEFAULT_BODY_BUFFER_SIZE: usize = 8192;

/// Reader of the data of a request body, see [accept]
pub type RequestBodyReader<T> = StreamReader<
    BodyDataStream<MapErr<T, fn(<T as http_body::Body>::Error) -> std::io::Error>>,
    <T as http_body::Body>::Data,
>;

/// Reader of the data of a response body, see [response]
pub type ResponseBodyReader<'a> = StreamReader<BoxStream<'a, std::io::Result<Bytes>>, Bytes>;

pin_project! {
    /// Body streaming the data written to the [DuplexStream] returned by [`OutgoingBody::new`]
    #[project = OutgoingBodyProj]
    #[derive(Debug)]
    pub struct OutgoingBody {
        buffer: BytesMut,
        #[pin]
        stream: DuplexStream,
    }
}

impl OutgoingBody {
    /// Returns the body and the writer of its data
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

/// Accepts a wRPC request with `body`, returning the response to send without waiting for the
/// request body, the response body writer and the request body reader
///
/// Read the request body to its end before dropping its reader: dropping it earlier may cancel
/// the response before the peer received it.
pub fn accept<T>(
    body: T,
) -> (
    http::Response<OutgoingBody>,
    DuplexStream,
    RequestBodyReader<T>,
)
where
    T: http_body::Body,
    T::Error: Into<Box<dyn core::error::Error + Send + Sync>>,
{
    let rx = StreamReader::new(body.map_err(io_error as fn(_) -> _).into_data_stream());
    let (body, tx) = OutgoingBody::new();
    (http::Response::new(body), tx, rx)
}

/// Returns a wRPC request with `parts` and its body writer
#[must_use]
pub fn request(mut parts: http::request::Parts) -> (http::Request<OutgoingBody>, DuplexStream) {
    let (body, tx) = OutgoingBody::new();
    parts.method = http::Method::POST;
    (http::Request::from_parts(parts, body), tx)
}

/// Returns a reader of the body of the response `res` resolves to.
///
/// The response is awaited on first read, so that the request body can be sent first.
/// Response status codes other than 2xx surface as an [`std::io::Error`] on read, carrying the
/// status and the first 4 KiB of the response body.
pub fn response<'a>(
    res: impl Future<
        Output = Result<
            http::Response<
                impl http_body::Body<
                    Data = Bytes,
                    Error: Into<Box<dyn core::error::Error + Send + Sync>>,
                > + Send
                + 'a,
            >,
            impl Into<Box<dyn core::error::Error + Send + Sync>>,
        >,
    > + Send
    + 'a,
) -> ResponseBodyReader<'a> {
    let body = stream::once(Box::pin(async move {
        let res = res.await.map_err(io_error)?;
        let (http::response::Parts { status, .. }, body) = res.into_parts();
        let body = body.map_err(io_error).into_data_stream();
        if !status.is_success() {
            let mut buf = Vec::default();
            let _ = pin!(StreamReader::new(body))
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
        Ok(body)
    }))
    .try_flatten();
    StreamReader::new(body.boxed())
}

/// Serves wRPC requests on HTTP/2 connection `io`, calling `f` with the parts of each and the
/// byte streams [accept] returns for its body
///
/// As with [accept], read each request body to its end before dropping its reader.
#[cfg(feature = "server")]
pub async fn serve(
    io: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    f: impl Fn(http::request::Parts, DuplexStream, RequestBodyReader<hyper::body::Incoming>),
) -> Result<(), hyper::Error> {
    hyper::server::conn::http2::Builder::new(hyper_util::rt::TokioExecutor::default())
        .serve_connection(
            hyper_util::rt::TokioIo::new(io),
            hyper::service::service_fn(move |req| {
                let (parts, body) = req.into_parts();
                let (res, tx, rx) = accept(body);
                f(parts, tx, rx);
                core::future::ready(Ok::<_, core::convert::Infallible>(res))
            }),
        )
        .await
}

/// Sends a wRPC request with `parts` on `sender`, returning the request body writer and the
/// response body reader
#[cfg(all(feature = "client", feature = "http2"))]
#[tracing::instrument(level = "trace", skip_all)]
pub async fn open(
    mut sender: hyper::client::conn::http2::SendRequest<OutgoingBody>,
    parts: http::request::Parts,
) -> Result<(DuplexStream, ResponseBodyReader<'static>), hyper::Error> {
    sender.ready().await?;
    let (req, tx) = request(parts);
    let rx = response(sender.send_request(req));
    Ok((tx, rx))
}

fn io_error(err: impl Into<Box<dyn core::error::Error + Send + Sync>>) -> std::io::Error {
    let err = match err.into().downcast::<std::io::Error>() {
        Ok(err) => return *err,
        Err(err) => err,
    };
    #[cfg(any(feature = "client", feature = "server"))]
    if err
        .downcast_ref::<hyper::Error>()
        .is_some_and(hyper::Error::is_timeout)
    {
        return std::io::Error::new(std::io::ErrorKind::TimedOut, err);
    }
    std::io::Error::other(err)
}
