#![allow(clippy::type_complexity)]

//! wRPC HTTP transport

use core::future::poll_fn;
use core::pin::{Pin, pin};
use core::task::{Context, Poll, ready};

use bytes::{Bytes, BytesMut};
use futures::channel::oneshot;
use futures::{Stream, StreamExt as _, TryStreamExt as _};
use http::HeaderValue;
use http::header::CONTENT_TYPE;
use http_body::{Frame, SizeHint};
use http_body_util::combinators::{MapErr, UnsyncBoxBody};
use http_body_util::{BodyDataStream, BodyExt};
use tokio::io::{AsyncRead, ReadBuf};
use tokio_util::codec::{Decoder, Encoder, FramedRead};
use tokio_util::io::StreamReader;
use wrpc_transport::{
    Codec, FrameDecoder, FrameEncoder, FrameRef, Header, HeaderDecoder, HeaderEncoder, HeaderRef,
    Invoke, Receipt, Receive, Serve, Transmit, error_data_after_end, receive,
};

/// Media type of wRPC request and response bodies
pub const MEDIA_TYPE: &str = "application/vnd.bytecodealliance.wrpc";

/// Size of the buffered data, at which a [Body] stops transmitting until the data is taken
const MAX_BUFFERED: usize = 1 << 16;

/// Reader of the data of a request or response body
pub type BodyReader<T> = StreamReader<
    BodyDataStream<MapErr<T, fn(<T as http_body::Body>::Error) -> std::io::Error>>,
    <T as http_body::Body>::Data,
>;

/// Body transmitting a value, the parameters of a [Client] request or the results of a [Server]
/// response, and the asynchronous values nested in it, using `E`
pub struct Body<E> {
    enc: E,
    path: Vec<usize>,
    buf: BytesMut,
    sent: SentSignal,
}

impl<E> Unpin for Body<E> {}

impl<E> Body<E> {
    fn new<T>(mut enc: E, header: Option<HeaderRef<'_>>, value: T) -> std::io::Result<Self>
    where
        E: Encoder<T, Error = std::io::Error>,
    {
        let mut buf = BytesMut::default();
        if let Some(header) = header {
            HeaderEncoder.encode(header, &mut buf)?;
        }
        let mut data = BytesMut::default();
        enc.encode(value, &mut data)?;
        for data in data.chunks(usize::try_from(u32::MAX).unwrap_or(usize::MAX)) {
            FrameEncoder.encode(FrameRef { path: &[], data }, &mut buf)?;
        }
        FrameEncoder.encode(
            FrameRef {
                path: &[],
                data: &[],
            },
            &mut buf,
        )?;
        Ok(Self {
            enc,
            path: Vec::default(),
            buf,
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
            buf,
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
        let mut closed = enc.is_closed();
        while !closed && buf.len() < MAX_BUFFERED {
            let n = buf.len();
            match enc.poll_transmit(cx, path, buf) {
                Poll::Ready(Ok(())) => closed = true,
                Poll::Ready(Err(err)) => return Poll::Ready(Some(Err(err))),
                Poll::Pending if buf.len() == n => break,
                Poll::Pending => {}
            }
        }
        sent.done = closed;
        if !buf.is_empty() {
            Poll::Ready(Some(Ok(Frame::data(buf.split().freeze()))))
        } else if closed {
            Poll::Ready(None)
        } else {
            Poll::Pending
        }
    }

    fn is_end_stream(&self) -> bool {
        self.buf.is_empty() && self.enc.is_closed()
    }

    fn size_hint(&self) -> SizeHint {
        let n = u64::try_from(self.buf.len()).unwrap_or(u64::MAX);
        if self.enc.is_closed() {
            SizeHint::with_exact(n)
        } else {
            let mut hint = SizeHint::default();
            hint.set_lower(n);
            hint
        }
    }
}

/// Signal of a request body, which was handed out in full
#[derive(Default)]
struct SentSignal {
    tx: Option<oneshot::Sender<()>>,
    done: bool,
}

impl Drop for SentSignal {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take()
            && self.done
        {
            _ = tx.send(());
        }
    }
}

/// Receives the invocation header of a request with `body`, returning it along with the [Server]
/// of the invocation
///
/// Receive the parameters from its frames to their end before dropping them: dropping them
/// earlier may cancel the response before the peer received it.
pub async fn accept<T>(body: T) -> std::io::Result<(Header, Server<T>)>
where
    T: http_body::Body<Error: Into<Box<dyn core::error::Error + Send + Sync>>> + Unpin,
{
    let rx = StreamReader::new(body.map_err(into_io_error as fn(_) -> _).into_data_stream());
    let mut rx = FramedRead::new(rx, HeaderDecoder::default());
    let Some(header) = rx.try_next().await? else {
        return Err(std::io::ErrorKind::UnexpectedEof.into());
    };
    let rx = rx.map_decoder(|_| FrameDecoder::default());
    Ok((header, Server { rx }))
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

/// Client invoking a function with a `POST` request with `parts`, with content type
/// [`MEDIA_TYPE`] unless `parts` set one, sent by `send`, such as a closure calling
/// `send_request` of a hyper HTTP/1 or HTTP/2 connection
///
/// The parameters are encoded before [`Invoke::invoke`] returns.
///
/// The response status and headers are not checked: to act on them, map the response `send`
/// resolves to, failing it with an error.
pub struct Client<F> {
    /// Parts of the request
    pub parts: http::request::Parts,
    /// Function sending the request and returning a future resolving to its response
    pub send: F,
}

impl<E, F, Fut, B, Err> Invoke<E> for Client<F>
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
        let body = Body::new(
            enc,
            Some(HeaderRef {
                instance,
                name: func,
            }),
            params,
        );
        parts.method = http::Method::POST;
        parts
            .headers
            .entry(CONTENT_TYPE)
            .or_insert(HeaderValue::from_static(MEDIA_TYPE));
        async move {
            let mut body = body?;
            let (tx, sent) = oneshot::channel();
            body.sent.tx = Some(tx);
            let res = send(http::Request::from_parts(parts, body));
            let rx = ResponseReader {
                res: Some(Box::pin(res)),
                body: None,
            };
            let rx = FramedRead::new(rx, FrameDecoder::default());
            let (results, rx) = receive(dec, rx).await?;
            Ok((
                results,
                Invocation {
                    sent: Some(sent),
                    rx: Some(rx),
                    rest: None,
                },
            ))
        }
    }
}

/// Future receiving the asynchronous results and the end of the response body of a [Client]
/// invocation
///
/// It also completes once the request body was sent, dropping it before fails the request body.
#[must_use]
pub struct Invocation<F, B: http_body::Body, D> {
    sent: Option<oneshot::Receiver<()>>,
    rx: Option<Receipt<FramedRead<ResponseReader<F, B>, FrameDecoder>, ResponseReader<F, B>, D>>,
    rest: Option<FramedRead<ResponseReader<F, B>, FrameDecoder>>,
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
        let Self { sent, rx, rest } = self.get_mut();
        if let Some(fut) = rx
            && let Poll::Ready(res) = Pin::new(fut).poll(cx)
        {
            res?;
            *rest = rx.take().map(Receipt::into_inner);
        }
        if let Poll::Ready(Err(err)) = poll_eof(cx, rest) {
            return Poll::Ready(Err(err));
        }
        if let Some(fut) = sent
            && let Poll::Ready(res) = Pin::new(fut).poll(cx)
        {
            res.map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::BrokenPipe, "request body was not sent")
            })?;
            *sent = None;
        }
        if sent.is_some() || rx.is_some() || rest.is_some() {
            return Poll::Pending;
        }
        Poll::Ready(Ok(()))
    }
}

/// Server serving the invocation, which a request body carries, see [accept], with the response
/// transmitting the results
///
/// The rest of the request body is received while the response body is polled, which fails on
/// an ingress error or on data following the invocation.
pub struct Server<T: http_body::Body> {
    /// Frames of the invocation following its header
    pub rx: FramedRead<BodyReader<T>, FrameDecoder>,
}

impl<T> Server<T>
where
    T: http_body::Body<Error: Into<Box<dyn core::error::Error + Send + Sync>>> + Unpin,
{
    /// Serves the invocation using `codec` by calling `f` with its parameters, see
    /// [`Codec::serve`], returning the response transmitting the results
    ///
    /// Unlike [`Serve`], this does not require [`Send`] values and futures.
    pub async fn serve<E, D, R, Err>(
        self,
        codec: Codec<E, D>,
        f: impl AsyncFnOnce(D::Item) -> Result<R, Err>,
    ) -> Result<http::Response<Body<ResultsEncoder<E, D, T>>>, Err>
    where
        E: Transmit + Encoder<R, Error = std::io::Error>,
        D: Decoder<Error = std::io::Error> + Receive,
        Err: From<std::io::Error>,
    {
        let Self { rx } = self;
        let Codec { enc, dec } = codec;
        let (params, rx) = receive(dec, rx).await?;
        let mut rx = Some(rx);
        let mut rest = None;
        let results = {
            let mut results = pin!(f(params));
            poll_fn(|cx| {
                if let Some(fut) = &mut rx
                    && let Poll::Ready(res) = Pin::new(fut).poll(cx)
                {
                    res?;
                    rest = rx.take().map(Receipt::into_inner);
                }
                results.as_mut().poll(cx)
            })
            .await?
        };
        let body = Body::new(ResultsEncoder { enc, rx, rest }, None, results)?;
        let mut res = http::Response::new(body);
        res.headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static(MEDIA_TYPE));
        Ok(res)
    }
}

impl<T> Serve for Server<T>
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
    ) -> impl Future<Output = Result<Self::Output, Err>> + Send
    where
        E: Transmit + Encoder<R, Error = std::io::Error> + Send + 'static,
        D: Decoder<Error = std::io::Error> + Receive + Send + 'static,
        D::Item: Send,
        Fut: Future<Output = Result<R, Err>> + Send,
        Err: From<std::io::Error>,
    {
        let res = Server::serve(self, codec, async move |params| f(params).await);
        async move {
            let res = res.await?;
            Ok(res.map(BodyExt::boxed_unsync))
        }
    }
}

/// Encoder of the results [`Server::serve`] responds with, which receives the asynchronous
/// parameters and the end of the request body while transmitting
pub struct ResultsEncoder<E, D, T: http_body::Body> {
    enc: E,
    rx: Option<Receipt<FramedRead<BodyReader<T>, FrameDecoder>, BodyReader<T>, D>>,
    rest: Option<FramedRead<BodyReader<T>, FrameDecoder>>,
}

impl<E, D, T, R> Encoder<R> for ResultsEncoder<E, D, T>
where
    E: Encoder<R, Error = std::io::Error>,
    T: http_body::Body,
{
    type Error = std::io::Error;

    fn encode(&mut self, item: R, dst: &mut BytesMut) -> std::io::Result<()> {
        self.enc.encode(item, dst)
    }
}

impl<E, D, T> Transmit for ResultsEncoder<E, D, T>
where
    E: Transmit,
    T: http_body::Body<Error: Into<Box<dyn core::error::Error + Send + Sync>>> + Unpin,
    D: Receive,
{
    fn poll_transmit(
        &mut self,
        cx: &mut Context<'_>,
        path: &mut Vec<usize>,
        dst: &mut BytesMut,
    ) -> Poll<std::io::Result<()>> {
        if let Some(rx) = &mut self.rx
            && let Poll::Ready(res) = Pin::new(rx).poll(cx)
        {
            res?;
            self.rest = self.rx.take().map(Receipt::into_inner);
        }
        if let Poll::Ready(Err(err)) = poll_eof(cx, &mut self.rest) {
            return Poll::Ready(Err(err));
        }
        if !self.enc.is_closed() {
            ready!(self.enc.poll_transmit(cx, path, dst))?;
        }
        if self.rx.is_some() || self.rest.is_some() {
            return Poll::Pending;
        }
        Poll::Ready(Ok(()))
    }

    fn is_closed(&self) -> bool {
        self.rx.is_none() && self.rest.is_none() && self.enc.is_closed()
    }
}

/// Serves wRPC requests on HTTP/2 connection `io`, calling `f` with the parts of each and the
/// invocation header and [Server] [accept] returns for its body, and sending the response `f`
/// resolves to
///
/// The stream of a request is reset if [accept] or `f` fails.
#[cfg(feature = "server")]
pub fn serve<B, Fut, Err>(
    io: impl hyper::rt::Read + hyper::rt::Write + Unpin,
    f: impl Fn(http::request::Parts, Header, Server<hyper::body::Incoming>) -> Fut
    + Clone
    + Send
    + 'static,
) -> impl Future<Output = Result<(), hyper::Error>>
where
    B: http_body::Body<Data = Bytes, Error: Into<Box<dyn core::error::Error + Send + Sync>>>
        + Send
        + 'static,
    Fut: Future<Output = Result<http::Response<B>, Err>> + Send + 'static,
    Err: Into<Box<dyn core::error::Error + Send + Sync>>,
{
    hyper::server::conn::http2::Builder::new(hyper_util::rt::TokioExecutor::default())
        .serve_connection(
            io,
            hyper::service::service_fn(move |req| {
                let f = f.clone();
                async move {
                    let (parts, body) = req.into_parts();
                    let (header, srv) = accept(body).await?;
                    let res = f(parts, header, srv).await.map_err(Into::into)?;
                    Ok::<_, Box<dyn core::error::Error + Send + Sync>>(res)
                }
            }),
        )
}

/// Polls `rx` for its end, failing on data
fn poll_eof(
    cx: &mut Context<'_>,
    rx: &mut Option<impl Stream<Item = std::io::Result<BytesMut>> + Unpin>,
) -> Poll<std::io::Result<()>> {
    if let Some(frames) = rx {
        let frame = ready!(frames.poll_next_unpin(cx)).transpose()?;
        if frame.is_some() {
            return Poll::Ready(Err(error_data_after_end()));
        }
        *rx = None;
    }
    Poll::Ready(Ok(()))
}

fn into_io_error(err: impl Into<Box<dyn core::error::Error + Send + Sync>>) -> std::io::Error {
    let err = match err.into().downcast::<std::io::Error>() {
        Ok(err) => return *err,
        Err(err) => err,
    };
    #[cfg(feature = "server")]
    if err
        .downcast_ref::<hyper::Error>()
        .is_some_and(hyper::Error::is_timeout)
    {
        return std::io::Error::new(std::io::ErrorKind::TimedOut, err);
    }
    std::io::Error::other(err)
}
