#![allow(clippy::type_complexity)]
#![deny(missing_docs)]

//! wRPC HTTP transport

use core::future::poll_fn;
use core::iter;
use core::mem;
use core::pin::{Pin, pin};
use core::task::{Context, Poll, ready};

use bytes::{Bytes, BytesMut};
use futures::channel::oneshot;
use futures::future::BoxFuture;
use futures::{Stream, StreamExt as _, TryFuture, TryFutureExt as _, TryStreamExt as _};
use http::HeaderValue;
use http::header::CONTENT_TYPE;
use http_body::{Frame, SizeHint};
use http_body_util::combinators::{MapErr, UnsyncBoxBody};
use http_body_util::{BodyDataStream, BodyExt};
use hyper::server::conn::{http1, http2};
use hyper_util::rt::TokioExecutor;
use hyper_util::server::conn::auto;
use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::mpsc;
use tokio_util::codec::{Decoder, Encoder, FramedRead};
use tokio_util::io::StreamReader;
use wrpc_core::{
    Codec, FrameDecoder, FrameEncoder, FrameRef, Header, HeaderDecoder, HeaderEncoder, HeaderRef,
    Invoke, Receipt, Receive, ReceiveError, Serve, ServeError, Transmit, error_data_after_end,
    receive,
};

/// Media type of wRPC request and response bodies
pub const MEDIA_TYPE: &str = "application/vnd.bytecodealliance.wrpc";

/// Reader of the data of a request or response body
pub type BodyReader<T> = StreamReader<
    BodyDataStream<MapErr<T, fn(<T as http_body::Body>::Error) -> std::io::Error>>,
    <T as http_body::Body>::Data,
>;

/// Body transmitting a value, the parameters of an [OutgoingRequest], and the asynchronous values
/// nested in it, using `E`
pub struct Body<E> {
    enc: E,
    path: Vec<usize>,
    buffer: BytesMut,
    sent: SentSignal,
}

impl<E> Unpin for Body<E> {}

impl<E> Body<E> {
    fn new<T>(mut enc: E, mut buffer: BytesMut, value: T) -> std::io::Result<Self>
    where
        E: Encoder<T, Error = std::io::Error>,
    {
        let mut data = BytesMut::default();
        enc.encode(value, &mut data)?;
        for data in data.chunks(usize::try_from(u32::MAX).unwrap_or(usize::MAX)) {
            FrameEncoder.encode(FrameRef { path: &[], data }, &mut buffer)?;
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
            buffer,
            sent: SentSignal::default(),
        })
    }
}

impl<E: Transmit> http_body::Body for Body<E> {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let Self {
            enc,
            path,
            buffer,
            sent,
        } = self.get_mut();
        if !sent.done
            && let Some(tx) = &mut sent.tx
            && tx.poll_canceled(cx).is_ready()
        {
            return Poll::Ready(Some(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "invocation dropped",
            ))));
        }
        let complete = enc.is_complete()
            || match enc.poll_transmit(cx, path, buffer) {
                Poll::Ready(Ok(())) => true,
                Poll::Ready(Err(err)) => {
                    let Some(tx) = sent.tx.take() else {
                        return Poll::Ready(Some(Err(err)));
                    };
                    let kind = err.kind();
                    if let Err(Err(err)) = tx.send(Err(err)) {
                        return Poll::Ready(Some(Err(err)));
                    }
                    return Poll::Ready(Some(Err(std::io::Error::new(kind, "body failed"))));
                }
                Poll::Pending => false,
            };
        sent.done = complete;
        if !buffer.is_empty() {
            Poll::Ready(Some(Ok(Frame::data(buffer.split().freeze()))))
        } else if complete {
            Poll::Ready(None)
        } else {
            Poll::Pending
        }
    }

    fn is_end_stream(&self) -> bool {
        self.buffer.is_empty() && self.enc.is_complete()
    }

    fn size_hint(&self) -> SizeHint {
        let n = u64::try_from(self.buffer.len()).unwrap_or(u64::MAX);
        if self.enc.is_complete() {
            SizeHint::with_exact(n)
        } else {
            let mut hint = SizeHint::default();
            hint.set_lower(n);
            hint
        }
    }
}

/// Signal of a body, which was handed out in full or failed
#[derive(Default)]
struct SentSignal {
    tx: Option<oneshot::Sender<std::io::Result<()>>>,
    done: bool,
}

impl Drop for SentSignal {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take()
            && self.done
        {
            _ = tx.send(Ok(()));
        }
    }
}

/// Receives the invocation header of a request with `body`, returning it along with the
/// [IncomingRequest] of the invocation
pub async fn accept<T>(body: T) -> std::io::Result<(Header, IncomingRequest<T>)>
where
    T: http_body::Body<Error: Into<Box<dyn core::error::Error + Send + Sync>>> + Unpin,
{
    let rx = StreamReader::new(body.map_err(into_io_error as fn(_) -> _).into_data_stream());
    let mut rx = FramedRead::new(rx, HeaderDecoder::default());
    let Some(header) = rx.try_next().await? else {
        return Err(std::io::ErrorKind::UnexpectedEof.into());
    };
    let rx = rx.map_decoder(|_| FrameDecoder::default());
    let (parts, ()) = http::Response::new(()).into_parts();
    Ok((header, IncomingRequest { rx, parts }))
}

/// Reader of the data of the body of the response a future resolves to, which it awaits on first
/// read
struct ResponseReader<F, B: http_body::Body> {
    res: Option<Pin<Box<F>>>,
    body: Option<Pin<Box<BodyReader<B>>>>,
}

impl<F, B, Err> AsyncRead for ResponseReader<F, B>
where
    F: Future<Output = Result<http::Response<B>, Err>>,
    B: http_body::Body<Data = Bytes, Error: Into<Box<dyn core::error::Error + Send + Sync>>>,
    Err: Into<Box<dyn core::error::Error + Send + Sync>>,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let Self { res, body } = self.get_mut();
        if let Some(fut) = res {
            let ret = ready!(fut.as_mut().poll(cx));
            *res = None;
            let ret = ret.map_err(into_io_error)?;
            *body = Some(Box::pin(StreamReader::new(
                ret.into_body()
                    .map_err(into_io_error as fn(_) -> _)
                    .into_data_stream(),
            )));
        }
        match body {
            Some(body) => body.as_mut().poll_read(cx, buf),
            None => Poll::Ready(Ok(())),
        }
    }
}

/// Outgoing `POST` request with `parts` invoking a function, with content type
/// [`MEDIA_TYPE`] unless `parts` set one, sent by `send`, such as a closure calling
/// `send_request` of a hyper HTTP/1 or HTTP/2 connection
///
/// The parameters are encoded before [`Invoke::invoke`] returns.
///
/// The response status and headers are not checked: to act on them, map the response `send`
/// resolves to, failing it with an error.
pub struct OutgoingRequest<F> {
    /// Parts of the request
    pub parts: http::request::Parts,
    /// Function sending the request and returning a future resolving to its response
    pub send: F,
}

impl<E, F, Fut, B, Err> Invoke<E> for OutgoingRequest<F>
where
    E: Transmit,
    F: FnOnce(http::Request<Body<E>>) -> Fut,
    Fut: Future<Output = Result<http::Response<B>, Err>>,
    B: http_body::Body<Data = Bytes, Error: Into<Box<dyn core::error::Error + Send + Sync>>>,
    Err: Into<Box<dyn core::error::Error + Send + Sync>>,
{
    type Invocation<D: Receive> = Invocation<Fut, B, D>;

    fn invoke<D, T>(
        self,
        codec: Codec<E, D>,
        instance: &str,
        func: &str,
        params: T,
    ) -> impl Future<Output = std::io::Result<(D::Item, Self::Invocation<D>)>>
    + use<E, F, Fut, B, Err, D, T>
    where
        E: Encoder<T, Error = std::io::Error>,
        D: Decoder<Error = std::io::Error> + Receive,
    {
        let Self { mut parts, send } = self;
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
            .and_then(|()| Body::new(enc, buffer, params));
        parts.method = http::Method::POST;
        parts
            .headers
            .entry(CONTENT_TYPE)
            .or_insert(HeaderValue::from_static(MEDIA_TYPE));
        async move {
            let mut body = body?;
            let (tx, mut sent) = oneshot::channel();
            body.sent.tx = Some(tx);
            let res = send(http::Request::from_parts(parts, body));
            let rx = ResponseReader {
                res: Some(Box::pin(res)),
                body: None,
            };
            let rx = FramedRead::new(rx, FrameDecoder::default());
            let (results, rx) = match receive(dec, rx).await {
                Ok(res) => res,
                Err(err) => {
                    if let Ok(Some(Err(err))) = sent.try_recv() {
                        return Err(err);
                    }
                    return Err(err.into());
                }
            };
            Ok((
                results,
                Invocation {
                    sent: Some(sent),
                    rx: Ingress::Receiving(rx),
                },
            ))
        }
    }
}

/// Future receiving the asynchronous results and the end of the response body of an
/// [OutgoingRequest] invocation
///
/// It also completes once the request body was sent, failing with its error, and dropping it
/// before fails the request body.
#[must_use]
pub struct Invocation<F, B: http_body::Body, D> {
    sent: Option<oneshot::Receiver<std::io::Result<()>>>,
    rx: Ingress<D, ResponseReader<F, B>>,
}

impl<F, B, Err, D> Future for Invocation<F, B, D>
where
    F: Future<Output = Result<http::Response<B>, Err>>,
    B: http_body::Body<Data = Bytes, Error: Into<Box<dyn core::error::Error + Send + Sync>>>,
    Err: Into<Box<dyn core::error::Error + Send + Sync>>,
    D: Receive,
{
    type Output = std::io::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Self { sent, rx } = self.get_mut();
        let canceled = poll_sent(cx, sent)?;
        let end = match Pin::new(rx).poll(cx) {
            Poll::Ready(Ok(())) => true,
            Poll::Ready(Err(err)) => return Poll::Ready(Err(sent_error(sent, err.into()))),
            Poll::Pending => false,
        };
        if canceled && end {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "request body was not sent",
            )));
        }
        if sent.is_some() || !end {
            return Poll::Pending;
        }
        Poll::Ready(Ok(()))
    }
}

/// Incoming request carrying an invocation, see [accept], served by responding with the
/// results, with content type [`MEDIA_TYPE`] unless `parts` set one
pub struct IncomingRequest<T: http_body::Body> {
    /// Frames of the invocation following its header
    pub rx: FramedRead<BodyReader<T>, FrameDecoder>,
    /// Parts of the response
    pub parts: http::response::Parts,
}

impl<T> IncomingRequest<T>
where
    T: http_body::Body<Error: Into<Box<dyn core::error::Error + Send + Sync>>> + Unpin,
{
    /// Serves the invocation using `codec` by calling `f` with its parameters, see
    /// [`Codec::serve`]
    ///
    /// Receives the asynchronous parameters while `f` runs and resolves to the response once the
    /// results `f` returns are encoded. Its [ResponseBody] transmits the asynchronous results and
    /// receives the rest of the request. A peer resetting the request fails with
    /// [`ServeError::Request`] of kind [`ConnectionReset`](std::io::ErrorKind::ConnectionReset).
    /// Data for an asynchronous parameter `f` dropped fails with [`ServeError::Unread`] until
    /// this resolves and fails the [ResponseBody] with
    /// [`BrokenPipe`](std::io::ErrorKind::BrokenPipe) after.
    ///
    /// Unlike [`Serve`], this does not require [`Send`] values and futures.
    pub async fn serve<E, D, R, Err>(
        self,
        codec: Codec<E, D>,
        f: impl AsyncFnOnce(D::Item) -> Result<R, Err>,
    ) -> Result<http::Response<ResponseBody<E, D, T>>, ServeError<Err>>
    where
        E: Transmit + Encoder<R, Error = std::io::Error>,
        D: Decoder<Error = std::io::Error> + Receive,
    {
        let Self { rx, mut parts } = self;
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
        let body = Body::new(enc, BytesMut::default(), results).map_err(ServeError::Response)?;
        parts
            .headers
            .entry(CONTENT_TYPE)
            .or_insert(HeaderValue::from_static(MEDIA_TYPE));
        Ok(http::Response::from_parts(parts, ResponseBody { body, rx }))
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

    fn serve<E, D, R, Fut, Err>(
        self,
        codec: Codec<E, D>,
        f: impl FnOnce(D::Item) -> Fut + Send,
    ) -> impl Future<Output = Result<Self::Output, ServeError<Err>>> + Send
    where
        E: Transmit + Encoder<R, Error = std::io::Error> + Send + 'static,
        D: Decoder<Error = std::io::Error> + Receive + Send + 'static,
        D::Item: Send,
        Fut: Future<Output = Result<R, Err>> + Send,
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
pub struct ResponseBody<E, D, T: http_body::Body> {
    body: Body<E>,
    rx: Ingress<D, BodyReader<T>>,
}

impl<E, D, T> http_body::Body for ResponseBody<E, D, T>
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

/// Request of an invocation yielded by a [Listener], which responds to it once served
pub struct ListenerRequest {
    /// Incoming request
    pub req: IncomingRequest<hyper::body::Incoming>,
    res: oneshot::Sender<http::Response<UnsyncBoxBody<Bytes, std::io::Error>>>,
}

impl ListenerRequest {
    /// Serves the invocation using `codec` by calling `f` with its parameters and responds with
    /// the results, see [`IncomingRequest::serve`]
    ///
    /// It resolves early once the peer canceled the request.
    ///
    /// Unlike [`Serve`], this does not require [`Send`] parameters and futures.
    pub async fn serve<E, D, R, Err>(
        self,
        codec: Codec<E, D>,
        f: impl AsyncFnOnce(D::Item) -> Result<R, Err>,
    ) -> Result<(), ServeError<Err>>
    where
        E: Transmit + Encoder<R, Error = std::io::Error> + Send + 'static,
        D: Decoder<Error = std::io::Error> + Receive + Send + 'static,
    {
        let Self { req, mut res } = self;
        let mut fut = pin!(req.serve(codec, f));
        let ret = poll_fn(|cx| {
            if res.poll_canceled(cx).is_ready() {
                return Poll::Ready(Ok(None));
            }
            fut.as_mut().poll(cx).map_ok(Some)
        })
        .await;
        let ret = match ret {
            Ok(Some(ret)) => ret,
            Ok(None) => return Ok(()),
            Err(ServeError::Request(err)) if err.kind() == std::io::ErrorKind::ConnectionReset => {
                return Ok(());
            }
            Err(err) => return Err(err),
        };
        _ = res.send(ret.map(BodyExt::boxed_unsync));
        Ok(())
    }
}

impl Serve for ListenerRequest {
    type Output = ();

    fn serve<E, D, R, Fut, Err>(
        self,
        codec: Codec<E, D>,
        f: impl FnOnce(D::Item) -> Fut + Send,
    ) -> impl Future<Output = Result<(), ServeError<Err>>> + Send
    where
        E: Transmit + Encoder<R, Error = std::io::Error> + Send + 'static,
        D: Decoder<Error = std::io::Error> + Receive + Send + 'static,
        D::Item: Send,
        Fut: Future<Output = Result<R, Err>> + Send,
    {
        ListenerRequest::serve(self, codec, async move |params| f(params).await)
    }
}

/// Incoming invocations of an HTTP/1 or HTTP/2 connection
///
/// It drives the connection while polled, so keep polling it while serving the invocations it
/// yields: forwarding it into a sink that waits for room, such as a bounded worker pool, stalls
/// the connection while the sink is full. It yields the parts of each request along with the
/// invocation header and the [ListenerRequest] of the [IncomingRequest] [accept] returns for its
/// body. Requests whose invocation header can't be read fail and their error is yielded, unless
/// the peer reset them. Requests whose [ListenerRequest] is dropped or fails before responding
/// fail. Failing a request resets its HTTP/2 stream, but fails an HTTP/1 connection. The error of
/// a failed connection is yielded last, after the items queued before it failed. Limits come from
/// the builder, such as HTTP/2
/// [`max_concurrent_streams`](auto::Http2Builder::max_concurrent_streams), beyond which streams are
/// refused with `REFUSED_STREAM`.
pub struct Listener<'a> {
    conn: Option<BoxFuture<'a, std::io::Result<()>>>,
    err: Option<std::io::Error>,
    rx: mpsc::Receiver<std::io::Result<(http::request::Parts, Header, ListenerRequest)>>,
}

impl<'a> Listener<'a> {
    /// Returns the incoming invocations of connection `io` configured by `builder`, serving
    /// HTTP/1 and HTTP/2 unless restricted by [`http1_only`](auto::Builder::http1_only) or
    /// [`http2_only`](auto::Builder::http2_only)
    pub fn new<I>(builder: &auto::Builder<TokioExecutor>, io: I) -> Self
    where
        I: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
    {
        Self::from_service(|svc| builder.serve_connection(io, svc).into_owned())
    }

    /// Returns the incoming invocations of HTTP/1 connection `io` configured by `builder`
    pub fn http1<I>(builder: &http1::Builder, io: I) -> Self
    where
        I: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'a,
    {
        Self::from_service(|svc| builder.serve_connection(io, svc))
    }

    /// Returns the incoming invocations of HTTP/2 connection `io` configured by `builder`
    pub fn http2<I>(builder: &http2::Builder<TokioExecutor>, io: I) -> Self
    where
        I: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'a,
    {
        Self::from_service(|svc| builder.serve_connection(io, svc))
    }

    fn from_service<T>(f: impl FnOnce(ListenerService) -> T) -> Self
    where
        T: TryFuture<Ok = (), Error: Into<Box<dyn core::error::Error + Send + Sync>>> + Send + 'a,
    {
        let (tx, rx) = mpsc::channel(1);
        Self {
            conn: Some(Box::pin(f(ListenerService { tx }).map_err(into_io_error))),
            err: None,
            rx,
        }
    }
}

impl Stream for Listener<'_> {
    type Item = std::io::Result<(http::request::Parts, Header, ListenerRequest)>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let Self { conn, err, rx } = self.get_mut();
        if let Some(fut) = conn
            && let Poll::Ready(res) = fut.as_mut().poll(cx)
        {
            *conn = None;
            *err = res.err();
        }
        let item = ready!(rx.poll_recv(cx));
        Poll::Ready(item.or_else(|| err.take().map(Err)))
    }
}

struct ListenerService {
    tx: mpsc::Sender<std::io::Result<(http::request::Parts, Header, ListenerRequest)>>,
}

impl hyper::service::Service<http::Request<hyper::body::Incoming>> for ListenerService {
    type Response = http::Response<UnsyncBoxBody<Bytes, std::io::Error>>;
    type Error = std::io::Error;
    type Future = BoxFuture<'static, std::io::Result<Self::Response>>;

    fn call(&self, req: http::Request<hyper::body::Incoming>) -> Self::Future {
        let tx = self.tx.clone();
        Box::pin(async move {
            let (parts, body) = req.into_parts();
            match accept(body).await {
                Ok((header, req)) => {
                    let (res, rx) = oneshot::channel();
                    tx.send(Ok((parts, header, ListenerRequest { req, res })))
                        .await
                        .map_err(|_| {
                            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "listener dropped")
                        })?;
                    rx.await.map_err(|oneshot::Canceled| {
                        std::io::Error::new(std::io::ErrorKind::BrokenPipe, "response was not sent")
                    })
                }
                Err(err) if err.kind() == std::io::ErrorKind::ConnectionReset => Err(err),
                Err(err) => {
                    let kind = err.kind();
                    _ = tx.send(Err(err)).await;
                    Err(std::io::Error::new(kind, "request failed"))
                }
            }
        })
    }
}

/// Polls `sent` for the body being sent, returning whether it was dropped before its end
fn poll_sent(
    cx: &mut Context<'_>,
    sent: &mut Option<oneshot::Receiver<std::io::Result<()>>>,
) -> std::io::Result<bool> {
    match sent.as_mut().map(|fut| Pin::new(fut).poll(cx)) {
        Some(Poll::Ready(Ok(res))) => {
            res?;
            *sent = None;
            Ok(false)
        }
        Some(Poll::Ready(Err(oneshot::Canceled))) => Ok(true),
        _ => Ok(false),
    }
}

/// Returns the error the body failed with, if any, else `err`
fn sent_error(
    sent: &mut Option<oneshot::Receiver<std::io::Result<()>>>,
    err: std::io::Error,
) -> std::io::Error {
    if let Some(Ok(Some(Err(err)))) = sent.as_mut().map(oneshot::Receiver::try_recv) {
        return err;
    }
    err
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
                return Poll::Ready(Err(error_data_after_end().into()));
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
    if err
        .downcast_ref::<hyper::Error>()
        .is_some_and(hyper::Error::is_timeout)
    {
        return std::io::Error::new(std::io::ErrorKind::TimedOut, err);
    }
    if iter::successors(Some(&*err as &dyn core::error::Error), |err| err.source())
        .filter_map(|err| err.downcast_ref::<h2::Error>())
        .any(|err| err.is_reset() && err.is_remote())
    {
        return std::io::Error::new(std::io::ErrorKind::ConnectionReset, err);
    }
    std::io::Error::other(err)
}
