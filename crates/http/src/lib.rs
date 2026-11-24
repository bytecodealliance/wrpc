#![deny(missing_docs)]

//! wRPC HTTP transport
//!
//! Servers [accept] the invocation header of a request and serve its [IncomingRequest], such as
//! in a hyper service:
//!
//! ```no_run
//! # async fn run(
//! #     io: impl hyper::rt::Read + hyper::rt::Write + Unpin,
//! # ) -> Result<(), Box<dyn core::error::Error + Send + Sync>> {
//! # #[derive(Debug)]
//! # struct EmptyName;
//! # impl core::fmt::Display for EmptyName {
//! #     fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
//! #         f.write_str("empty name")
//! #     }
//! # }
//! # impl core::error::Error for EmptyName {}
//! use bytes::Bytes;
//! use http::StatusCode;
//! use http_body_util::combinators::UnsyncBoxBody;
//! use hyper::body::Incoming;
//! use hyper::server::conn::http1;
//! use hyper::service::service_fn;
//! use wrpc_core::{Header, ServeError, serve};
//!
//! fn status(status: StatusCode) -> http::Response<UnsyncBoxBody<Bytes, std::io::Error>> {
//!     let mut res = http::Response::new(UnsyncBoxBody::default());
//!     *res.status_mut() = status;
//!     res
//! }
//!
//! let svc = service_fn(|req: http::Request<Incoming>| async move {
//!     let (Header { instance, name }, req) = wrpc_http::accept(req.into_body()).await?;
//!     if instance != "foo" || name != "bar" {
//!         return Ok(status(StatusCode::NOT_FOUND));
//!     }
//!     let res = match serve::<(String,), (String,), _, _>(req, async |(name,)| {
//!         if name.is_empty() {
//!             return Err(std::io::Error::other(EmptyName));
//!         }
//!         Ok((format!("Hello, {name}"),))
//!     })
//!     .await
//!     {
//!         Ok(res) => res,
//!         Err(ServeError::Handler(err))
//!             if err.get_ref().and_then(|err| err.downcast_ref::<EmptyName>()).is_some() =>
//!         {
//!             status(StatusCode::UNPROCESSABLE_ENTITY)
//!         }
//!         Err(err) => return Err(err.into()),
//!     };
//!     Ok::<_, Box<dyn core::error::Error + Send + Sync>>(res)
//! });
//! http1::Builder::new().serve_connection(io, svc).await?;
//! # Ok(())
//! # }
//! ```
//!
//! Clients invoke functions with a [Client] sending requests with a [`Service`], such as a
//! hyper-util client, here shared with other requests and failing on a non-2xx response status:
//!
//! ```no_run
//! # async fn run() -> Result<(), Box<dyn core::error::Error + Send + Sync>> {
//! use bytes::Bytes;
//! use http::{Request, Uri};
//! use http_body_util::BodyExt;
//! use http_body_util::combinators::UnsyncBoxBody;
//! use hyper_util::client::legacy;
//! use hyper_util::rt::TokioExecutor;
//! use tower::ServiceExt as _;
//! use wrpc_core::invoke;
//! use wrpc_http::{Client, OutgoingParams};
//!
//! let http: legacy::Client<_, UnsyncBoxBody<Bytes, std::io::Error>> =
//!     legacy::Client::builder(TokioExecutor::default()).build_http();
//! let svc = http
//!     .clone()
//!     .map_request(|req: Request<OutgoingParams>| req.map(BodyExt::boxed_unsync))
//!     .map_result(|res| -> Result<_, Box<dyn core::error::Error + Send + Sync>> {
//!         let res = res?;
//!         if !res.status().is_success() {
//!             return Err(format!("unexpected response status `{}`", res.status()).into());
//!         }
//!         Ok(res)
//!     });
//! let clt = Client::new(Uri::from_static("http://localhost:7763"), svc);
//! let name = String::from("world");
//! let ((greeting,), io) = invoke::<_, (String,), _>(clt, "foo", "bar", (name,)).await?;
//! io.await?;
//! println!("{greeting}");
//! # Ok(())
//! # }
//! ```
//!
//! The response status is left to callers: clients act on it in their [`Service`], servers may
//! set the status and headers of the response [`IncomingRequest::serve`] returns, `200 OK` by
//! default.
//!
//! With the `h2` feature (default), [`h2`](https://docs.rs/h2) errors from the peer are reported
//! as [`std::io::ErrorKind::ConnectionReset`], refused streams and graceful shutdowns as
//! [`std::io::ErrorKind::ConnectionRefused`].

use core::future::poll_fn;
use core::mem;
use core::pin::{Pin, pin};
use core::task::{Context, Poll, ready};

use std::collections::VecDeque;

use bytes::{Bytes, BytesMut};
use futures::channel::oneshot;
use futures::{StreamExt as _, TryFutureExt as _, TryStreamExt as _};
use http::header::CONTENT_TYPE;
use http::{HeaderValue, Method, Uri};
use http_body::{Frame, SizeHint};
use http_body_util::combinators::{MapErr, UnsyncBoxBody};
use http_body_util::{BodyDataStream, BodyExt};
use tokio::io::AsyncRead;
use tokio_util::codec::{Decoder, Encoder, FramedRead};
use tokio_util::io::StreamReader;
use wrpc_core::{
    Codec, FrameDecoder, FrameEncoder, FrameHeaderEncoder, FrameHeaderRef, FrameRef, Header,
    HeaderDecodeError, HeaderDecoder, HeaderEncoder, HeaderRef, Invoke, InvokeError, Receipt,
    Receive, ReceiveError, Serve, ServeError, Transmit, receive,
};

#[doc(inline)]
pub use tower_service::Service;

/// Media type of wRPC request and response bodies
pub const MEDIA_TYPE: &str = "application/vnd.bytecodealliance.wrpc";

/// Reader of the data of a request or response body
pub type BodyReader<T> = StreamReader<
    BodyDataStream<MapErr<T, fn(<T as http_body::Body>::Error) -> std::io::Error>>,
    <T as http_body::Body>::Data,
>;

/// Body of a [Client] request, transmitting the parameters with a [`Send`] encoder
pub struct OutgoingParams(Egress<Box<dyn Transmit + Send>>);

impl http_body::Body for OutgoingParams {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Pin::new(&mut self.get_mut().0).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.0.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.0.size_hint()
    }
}

/// Body transmitting a value and the asynchronous values nested in it using `E`
struct Egress<E> {
    enc: E,
    path: Vec<usize>,
    frames: VecDeque<Bytes>,
    buffer: BytesMut,
    error: Option<oneshot::Sender<std::io::Error>>,
}

impl<E> Unpin for Egress<E> {}

impl<E> Egress<E> {
    fn new<T>(mut enc: E, mut buffer: BytesMut, value: T) -> std::io::Result<Self>
    where
        E: Encoder<T, Error = std::io::Error>,
    {
        let mut data = BytesMut::default();
        enc.encode(value, &mut data)?;
        let mut data = data.freeze();
        let mut frames = VecDeque::default();
        while !data.is_empty() {
            let size = data
                .len()
                .min(usize::try_from(u32::MAX).unwrap_or(usize::MAX));
            FrameHeaderEncoder.encode(FrameHeaderRef { path: &[], size }, &mut buffer)?;
            frames.push_back(buffer.split().freeze());
            frames.push_back(data.split_to(size));
        }
        FrameEncoder.encode(
            FrameRef {
                path: &[],
                data: &[],
            },
            &mut buffer,
        )?;
        Ok(Self {
            enc,
            path: Vec::default(),
            frames,
            buffer,
            error: None,
        })
    }
}

impl<E: Transmit> http_body::Body for Egress<E> {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let Self {
            enc,
            path,
            frames,
            buffer,
            error,
        } = self.get_mut();
        if let Some(tx) = error
            && tx.poll_canceled(cx).is_ready()
        {
            return Poll::Ready(Some(Err(std::io::ErrorKind::BrokenPipe.into())));
        }
        if let Some(frame) = frames.pop_front() {
            return Poll::Ready(Some(Ok(Frame::data(frame))));
        }
        let complete = enc.is_complete()
            || match enc.poll_transmit(cx, path, buffer) {
                Poll::Ready(Ok(())) => true,
                Poll::Ready(Err(err)) => {
                    let Some(tx) = error.take() else {
                        return Poll::Ready(Some(Err(err)));
                    };
                    let kind = err.kind();
                    return Poll::Ready(Some(Err(match tx.send(err) {
                        Ok(()) => kind.into(),
                        Err(err) => err,
                    })));
                }
                Poll::Pending => false,
            };
        if complete {
            *error = None;
        }
        if !buffer.is_empty() {
            Poll::Ready(Some(Ok(Frame::data(buffer.split().freeze()))))
        } else if complete {
            Poll::Ready(None)
        } else {
            Poll::Pending
        }
    }

    fn is_end_stream(&self) -> bool {
        self.frames.is_empty() && self.buffer.is_empty() && self.enc.is_complete()
    }

    fn size_hint(&self) -> SizeHint {
        let n = self
            .frames
            .iter()
            .map(Bytes::len)
            .fold(self.buffer.len(), usize::saturating_add);
        let n = u64::try_from(n).unwrap_or(u64::MAX);
        if self.enc.is_complete() {
            SizeHint::with_exact(n)
        } else {
            let mut hint = SizeHint::default();
            hint.set_lower(n);
            hint
        }
    }
}

/// Receives the invocation header of a request with `body`, returning it along with the
/// [IncomingRequest] of the invocation
///
/// Pass `Box::pin(body)` for a `!Unpin` body.
pub async fn accept<T>(body: T) -> Result<(Header, IncomingRequest<T>), HeaderDecodeError>
where
    T: http_body::Body<Error: Into<Box<dyn core::error::Error + Send + Sync>>> + Unpin,
{
    let rx = StreamReader::new(body.map_err(into_io_error as fn(_) -> _).into_data_stream());
    let mut rx = FramedRead::new(rx, HeaderDecoder::default());
    let Some(header) = rx.try_next().await? else {
        return Err(HeaderDecodeError::Truncated);
    };
    let rx = rx.map_decoder(|_| FrameDecoder::default());
    let (response, ()) = http::Response::new(()).into_parts();
    Ok((header, IncomingRequest { rx, response }))
}

/// Client invoking functions with requests built from `parts`, sent by `service`, such as a
/// hyper-util client
///
/// The parameters are encoded before [`Invoke::invoke`] returns. The response status and headers
/// are not checked: to act on them, wrap `service` in a layer.
/// Errors of `service` are [`InvokeError::Request`].
#[derive(Clone, Debug)]
pub struct Client<T> {
    /// Parts of the requests
    pub parts: http::request::Parts,
    /// Service sending the requests
    pub service: T,
}

impl<T> Client<T> {
    /// Returns a client sending `POST` requests with content type [`MEDIA_TYPE`] to `uri` with
    /// `service`
    pub fn new(uri: Uri, service: T) -> Self {
        let (mut parts, ()) = http::Request::new(()).into_parts();
        parts.method = Method::POST;
        parts.uri = uri;
        parts
            .headers
            .insert(CONTENT_TYPE, HeaderValue::from_static(MEDIA_TYPE));
        Self { parts, service }
    }
}

impl<E, S, B> Invoke<E> for Client<S>
where
    E: Transmit + Send + 'static,
    S: Service<
            http::Request<OutgoingParams>,
            Response = http::Response<B>,
            Error: Into<Box<dyn core::error::Error + Send + Sync>>,
        >,
    B: http_body::Body<Data = Bytes, Error: Into<Box<dyn core::error::Error + Send + Sync>>>,
{
    type Invocation<D: Receive> = Invocation<B, D>;

    fn invoke<D, T>(
        self,
        codec: Codec<E, D>,
        instance: &str,
        func: &str,
        params: T,
    ) -> impl Future<Output = Result<(D::Item, Self::Invocation<D>), InvokeError>> + use<E, S, B, D, T>
    where
        E: Encoder<T, Error = std::io::Error>,
        D: Decoder<Error = std::io::Error> + Receive,
    {
        let Self { parts, mut service } = self;
        let Codec { enc, dec } = codec;
        let mut buffer = BytesMut::default();
        let body = HeaderEncoder
            .encode(
                HeaderRef {
                    instance,
                    name: func,
                },
                &mut buffer,
            )
            .and_then(|()| Egress::new(enc, buffer, params));
        async move {
            let Egress {
                enc,
                path,
                frames,
                buffer,
                ..
            } = body.map_err(InvokeError::Request)?;
            poll_fn(|cx| service.poll_ready(cx))
                .await
                .map_err(|err| InvokeError::Request(into_io_error(err)))?;
            let (tx, mut error) = oneshot::channel();
            let req = http::Request::from_parts(
                parts,
                OutgoingParams(Egress {
                    enc: Box::new(enc),
                    path,
                    frames,
                    buffer,
                    error: Some(tx),
                }),
            );
            let res = service.call(req).await.map_err(|err| {
                InvokeError::Request(match error.try_recv() {
                    Ok(Some(err)) => err,
                    _ => into_io_error(err),
                })
            })?;
            let rx = Box::pin(StreamReader::new(
                res.into_body()
                    .map_err(into_io_error as fn(_) -> _)
                    .into_data_stream(),
            ));
            let rx = FramedRead::new(rx, FrameDecoder::default());
            let (results, rx) = match receive(dec, rx).await {
                Ok(res) => res,
                Err(err) => {
                    if let Ok(Some(err)) = error.try_recv() {
                        return Err(InvokeError::Request(err));
                    }
                    return Err(err.into());
                }
            };
            Ok((
                results,
                Invocation {
                    error,
                    rx: Ingress::Receiving(rx),
                },
            ))
        }
    }
}

impl<'a, E, S, B> Invoke<E> for &'a mut Client<S>
where
    E: Transmit + Send + 'static,
    S: Service<
            http::Request<OutgoingParams>,
            Response = http::Response<B>,
            Error: Into<Box<dyn core::error::Error + Send + Sync>>,
        >,
    B: http_body::Body<Data = Bytes, Error: Into<Box<dyn core::error::Error + Send + Sync>>>,
{
    type Invocation<D: Receive> = Invocation<B, D>;

    fn invoke<D, T>(
        self,
        codec: Codec<E, D>,
        instance: &str,
        func: &str,
        params: T,
    ) -> impl Future<Output = Result<(D::Item, Self::Invocation<D>), InvokeError>> + use<'a, E, S, B, D, T>
    where
        E: Encoder<T, Error = std::io::Error>,
        D: Decoder<Error = std::io::Error> + Receive,
    {
        let Client { parts, service } = self;
        Client {
            parts: parts.clone(),
            service,
        }
        .invoke(codec, instance, func, params)
    }
}

/// Future receiving the asynchronous results and the end of the response body of a [Client]
/// invocation, failing with the error of its request body, if any
///
/// Dropping it before the request body was sent fails the request body.
#[must_use]
pub struct Invocation<B: http_body::Body, D> {
    error: oneshot::Receiver<std::io::Error>,
    rx: Ingress<D, Pin<Box<BodyReader<B>>>>,
}

impl<B, D> Future for Invocation<B, D>
where
    B: http_body::Body<Data = Bytes, Error: Into<Box<dyn core::error::Error + Send + Sync>>>,
    D: Receive,
{
    type Output = Result<(), InvokeError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Self { error, rx } = self.get_mut();
        if let Err(err) = ready!(Pin::new(rx).poll(cx)) {
            if let Ok(Some(err)) = error.try_recv() {
                return Poll::Ready(Err(InvokeError::Request(err)));
            }
            return Poll::Ready(Err(err.into()));
        }
        Poll::Ready(Ok(()))
    }
}

/// Incoming request carrying an invocation, see [accept], served by responding with the
/// results, with content type [`MEDIA_TYPE`] unless `response` sets one
pub struct IncomingRequest<T: http_body::Body> {
    /// Frames of the invocation following its header
    pub rx: FramedRead<BodyReader<T>, FrameDecoder>,
    /// Parts of the response
    pub response: http::response::Parts,
}

impl<T> IncomingRequest<T>
where
    T: http_body::Body<Error: Into<Box<dyn core::error::Error + Send + Sync>>> + Unpin,
{
    /// Serves the invocation using `codec` by calling `f` with its parameters, see
    /// [`Codec::serve`]
    ///
    /// Receives the asynchronous parameters while `f` runs and resolves to the response once the
    /// results `f` returns are encoded. Its [OutgoingResults] transmits the asynchronous results
    /// and receives the rest of the request. Data for an asynchronous parameter `f` dropped fails
    /// with [`ServeError::Unread`] until this resolves and fails the [OutgoingResults] with
    /// [`BrokenPipe`](std::io::ErrorKind::BrokenPipe) after.
    ///
    /// Unlike [`Serve`], this does not require [`Send`] values and futures.
    pub async fn serve<E, D, R>(
        self,
        codec: Codec<E, D>,
        f: impl AsyncFnOnce(D::Item) -> std::io::Result<R>,
    ) -> Result<http::Response<OutgoingResults<E, D, T>>, ServeError>
    where
        E: Transmit + Encoder<R, Error = std::io::Error>,
        D: Decoder<Error = std::io::Error> + Receive,
    {
        let Self { rx, mut response } = self;
        let Codec { enc, dec } = codec;
        let (params, rx) = receive(dec, rx).await?;
        let mut rx = Ingress::Receiving(rx);
        let mut results = pin!(f(params));
        let results = poll_fn(|cx| {
            if let Poll::Ready(Err(err)) = Pin::new(&mut rx).poll(cx) {
                return Poll::Ready(Err(err.into()));
            }
            results.as_mut().poll(cx).map_err(ServeError::Handler)
        })
        .await?;
        let body = Egress::new(enc, BytesMut::default(), results).map_err(ServeError::Response)?;
        response
            .headers
            .entry(CONTENT_TYPE)
            .or_insert(HeaderValue::from_static(MEDIA_TYPE));
        Ok(http::Response::from_parts(
            response,
            OutgoingResults { body, rx },
        ))
    }
}

impl<T> Serve for IncomingRequest<T>
where
    T: http_body::Body<Data: Send, Error: Into<Box<dyn core::error::Error + Send + Sync>>>
        + Unpin
        + Send
        + 'static,
{
    type Output = http::Response<UnsyncBoxBody<Bytes, std::io::Error>>;

    fn serve<E, D, R, Fut>(
        self,
        codec: Codec<E, D>,
        f: impl FnOnce(D::Item) -> Fut + Send,
    ) -> impl Future<Output = Result<Self::Output, ServeError>> + Send
    where
        E: Transmit + Encoder<R, Error = std::io::Error> + Send + 'static,
        D: Decoder<Error = std::io::Error> + Receive + Send + 'static,
        D::Item: Send,
        Fut: Future<Output = std::io::Result<R>> + Send,
    {
        IncomingRequest::serve(self, codec, async move |params| f(params).await)
            .map_ok(|res| res.map(BodyExt::boxed_unsync))
    }
}

/// Body of the response to an [IncomingRequest], transmitting the results using `E` and receiving
/// the rest of the request using `D`
///
/// It fails on the first error of either and ends once both are done. The rest of the request is
/// received only while it is polled, which HTTP/2 does only while the response has flow-control
/// capacity.
pub struct OutgoingResults<E, D, T: http_body::Body> {
    body: Egress<E>,
    rx: Ingress<D, BodyReader<T>>,
}

impl<E, D, T> http_body::Body for OutgoingResults<E, D, T>
where
    E: Transmit,
    D: Receive,
    T: http_body::Body<Error: Into<Box<dyn core::error::Error + Send + Sync>>> + Unpin,
{
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let Self { body, rx } = self.get_mut();
        if let Poll::Ready(Err(err)) = Pin::new(&mut *rx).poll(cx) {
            return Poll::Ready(Some(Err(err.into())));
        }
        match ready!(Pin::new(body).poll_frame(cx)) {
            None if !matches!(rx, Ingress::Done) => Poll::Pending,
            frame => Poll::Ready(frame),
        }
    }

    fn is_end_stream(&self) -> bool {
        matches!(self.rx, Ingress::Done) && self.body.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        let hint = self.body.size_hint();
        if matches!(self.rx, Ingress::Done) {
            return hint;
        }
        let mut lower = SizeHint::default();
        lower.set_lower(hint.lower());
        lower
    }
}

/// Receipt of the rest of a body, followed by its end
enum Ingress<D, I> {
    Receiving(Receipt<D, FramedRead<I, FrameDecoder>, I>),
    Trailing(FramedRead<I, FrameDecoder>),
    Done,
}

impl<D: Receive, I: AsyncRead + Unpin> Future for Ingress<D, I> {
    type Output = Result<(), ReceiveError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if let Self::Receiving(fut) = this {
            ready!(Pin::new(fut).poll(cx))?;
            if let Self::Receiving(fut) = mem::replace(this, Self::Done) {
                *this = Self::Trailing(fut.into_inner());
            }
        }
        if let Self::Trailing(rx) = this {
            let frame = ready!(rx.poll_next_unpin(cx)).transpose()?;
            if frame.is_some() {
                if !rx.decoder().path().is_empty() {
                    return Poll::Ready(Err(ReceiveError::UnknownPath));
                }
                return Poll::Ready(Err(ReceiveError::DataAfterEnd));
            }
            *this = Self::Done;
        }
        Poll::Ready(Ok(()))
    }
}

fn into_io_error(err: impl Into<Box<dyn core::error::Error + Send + Sync>>) -> std::io::Error {
    let err = match err.into().downcast::<std::io::Error>() {
        Ok(err) => return *err,
        Err(err) => err,
    };
    #[cfg(feature = "h2")]
    if let Some(kind) =
        core::iter::successors(Some(&*err as &dyn core::error::Error), |err| err.source())
            .find_map(|err| err.downcast_ref::<h2::Error>())
            .filter(|err| err.is_remote())
            .map(|err| match err.reason() {
                Some(h2::Reason::REFUSED_STREAM) => std::io::ErrorKind::ConnectionRefused,
                Some(h2::Reason::NO_ERROR) if err.is_go_away() => {
                    std::io::ErrorKind::ConnectionRefused
                }
                _ => std::io::ErrorKind::ConnectionReset,
            })
    {
        return std::io::Error::new(kind, err);
    }
    std::io::Error::other(err)
}
