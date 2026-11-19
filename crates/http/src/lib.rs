//! wRPC HTTP transport

use core::convert::Infallible;
use core::pin::{Pin, pin};
use core::task::{Context, Poll, ready};

use bytes::Bytes;
use futures::stream::BoxStream;
use futures::{StreamExt as _, TryStreamExt as _, stream};
use http_body::{Frame, SizeHint};
use http_body_util::combinators::MapErr;
use http_body_util::{BodyDataStream, BodyExt as _, Full};
use tokio::io::{AsyncReadExt as _, AsyncWrite};
use tokio::sync::{mpsc, oneshot};
use tokio_util::io::StreamReader;

/// Reader of the data of a request body, see [accept]
pub type RequestBodyReader<T> = StreamReader<
    BodyDataStream<MapErr<T, fn(<T as http_body::Body>::Error) -> std::io::Error>>,
    <T as http_body::Body>::Data,
>;

/// Reader of the data of a response body, see [response]
pub type ResponseBodyReader<'a> = StreamReader<BoxStream<'a, std::io::Result<Bytes>>, Bytes>;

/// Body streaming the data written to the [`OutgoingBodyWriter`] returned by
/// [`OutgoingBody::new`], or a fixed body created by [`OutgoingBody::full`]
#[derive(Debug)]
pub struct OutgoingBody(OutgoingBodyInner);

#[derive(Debug)]
enum OutgoingBodyInner {
    Full(Full<Bytes>),
    Stream {
        demand: mpsc::Sender<oneshot::Sender<Bytes>>,
        data: Option<oneshot::Receiver<Bytes>>,
    },
}

impl OutgoingBody {
    /// Returns the body and the writer of its data
    ///
    /// Writes complete only once the body is polled for data.
    #[must_use]
    pub fn new() -> (Self, OutgoingBodyWriter) {
        let (tx, rx) = mpsc::channel(1);
        (
            Self(OutgoingBodyInner::Stream {
                demand: tx,
                data: None,
            }),
            OutgoingBodyWriter(Some(rx)),
        )
    }

    /// Returns a body of `data` with a known size, for sending plain requests with the same client
    #[must_use]
    pub fn full(data: impl Into<Bytes>) -> Self {
        Self(OutgoingBodyInner::Full(Full::new(data.into())))
    }
}

impl http_body::Body for OutgoingBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match &mut self.0 {
            OutgoingBodyInner::Full(body) => Pin::new(body).poll_frame(cx),
            OutgoingBodyInner::Stream { demand, data } => {
                let rx = match data {
                    Some(rx) => rx,
                    None => {
                        let (tx, rx) = oneshot::channel();
                        if demand.try_send(tx).is_err() {
                            return Poll::Ready(None);
                        }
                        data.insert(rx)
                    }
                };
                let buf = ready!(Pin::new(rx).poll(cx));
                *data = None;
                Poll::Ready(buf.ok().map(|buf| Ok(Frame::data(buf))))
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        match &self.0 {
            OutgoingBodyInner::Full(body) => body.is_end_stream(),
            OutgoingBodyInner::Stream { demand, data } => demand.is_closed() && data.is_none(),
        }
    }

    fn size_hint(&self) -> SizeHint {
        match &self.0 {
            OutgoingBodyInner::Full(body) => body.size_hint(),
            OutgoingBodyInner::Stream { .. } => SizeHint::default(),
        }
    }
}

/// Writer of the data of an [`OutgoingBody`], handing each write to the body as one chunk once
/// the body is polled for data
///
/// Dropping or shutting down the writer ends the body.
#[derive(Debug)]
pub struct OutgoingBodyWriter(Option<mpsc::Receiver<oneshot::Sender<Bytes>>>);

impl AsyncWrite for OutgoingBodyWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let Some(demand) = &mut self.0 else {
            return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
        };
        let tx = ready!(demand.poll_recv(cx)).ok_or(std::io::ErrorKind::BrokenPipe)?;
        tx.send(Bytes::copy_from_slice(buf))
            .map_err(|_| std::io::ErrorKind::BrokenPipe)?;
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.0 = None;
        Poll::Ready(Ok(()))
    }
}

/// Accepts a wRPC request with `body`, returning the response to send without waiting for the
/// request body, the response body writer and the request body reader
///
/// Writes to the response body writer complete only once the response body is polled. Read the
/// request body to its end before dropping its reader: dropping it earlier may cancel the
/// response before the peer received it.
pub fn accept<T>(
    body: T,
) -> (
    http::Response<OutgoingBody>,
    OutgoingBodyWriter,
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
///
/// Writes complete only once the request is sent and its body polled, so write it concurrently
/// with reading the [response].
#[must_use]
pub fn request(
    mut parts: http::request::Parts,
) -> (http::Request<OutgoingBody>, OutgoingBodyWriter) {
    let (body, tx) = OutgoingBody::new();
    parts.method = http::Method::POST;
    (http::Request::from_parts(parts, body), tx)
}

/// Returns a reader of the body of the response `res` resolves to.
///
/// `res` is only polled on read, so read concurrently with writing the request body.
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

/// Serves wRPC requests on HTTP/2 connection `io`, calling `f` with the parts of each, the parts
/// of its response (200 OK without headers by default) and the byte streams [accept] returns for
/// its body
///
/// The response is sent once `f` returns. As with [accept], read each request body to its end
/// before dropping its reader.
#[cfg(feature = "server")]
pub async fn serve(
    io: impl hyper::rt::Read + hyper::rt::Write + Unpin,
    f: impl Fn(
        http::request::Parts,
        &mut http::response::Parts,
        OutgoingBodyWriter,
        RequestBodyReader<hyper::body::Incoming>,
    ),
) -> Result<(), hyper::Error> {
    hyper::server::conn::http2::Builder::new(hyper_util::rt::TokioExecutor::default())
        .serve_connection(
            io,
            hyper::service::service_fn(move |req| {
                let (parts, body) = req.into_parts();
                let (res, tx, rx) = accept(body);
                let (mut res, body) = res.into_parts();
                f(parts, &mut res, tx, rx);
                let res = http::Response::from_parts(res, body);
                core::future::ready(Ok::<_, Infallible>(res))
            }),
        )
        .await
}

/// Sends a wRPC request with `parts` on `sender`, returning the request body writer and the
/// response body reader
///
/// As with [request], write the request body concurrently with reading the response body.
#[cfg(all(feature = "client", feature = "http2"))]
#[tracing::instrument(level = "trace", skip_all)]
pub async fn open(
    mut sender: hyper::client::conn::http2::SendRequest<OutgoingBody>,
    parts: http::request::Parts,
) -> Result<(OutgoingBodyWriter, ResponseBodyReader<'static>), hyper::Error> {
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
