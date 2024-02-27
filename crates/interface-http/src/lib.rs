use core::future::Future;
use core::pin::Pin;
use core::time::Duration;

use anyhow::{bail, Context as _};
use async_trait::async_trait;
use bytes::{Buf, BufMut, Bytes};
use futures::{Stream, StreamExt as _};
use tokio::try_join;
use tracing::instrument;
use wrpc_transport::{
    encode_discriminant, receive_discriminant, Acceptor, AsyncSubscription, AsyncValue, Encode,
    EncodeSync, Receive, StreamItem, Subject as _, Subscribe, Subscriber,
};

pub type Fields = Vec<(String, Vec<Bytes>)>;

fn fields_to_wrpc(fields: Vec<(String, Vec<Bytes>)>) -> wrpc_transport::Value {
    fields
        .into_iter()
        .map(|(k, v)| {
            (
                k.into(),
                v.into_iter().map(Into::into).collect::<Vec<_>>().into(),
            )
                .into()
        })
        .collect::<Vec<_>>()
        .into()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Method {
    Get,
    Head,
    Post,
    Put,
    Delete,
    Connect,
    Options,
    Trace,
    Patch,
    Other(String),
}

impl EncodeSync for Method {
    #[instrument(level = "trace", skip_all)]
    fn encode_sync(self, mut payload: impl BufMut) -> anyhow::Result<()> {
        match self {
            Self::Get => encode_discriminant(payload, 0),
            Self::Head => encode_discriminant(payload, 1),
            Self::Post => encode_discriminant(payload, 2),
            Self::Put => encode_discriminant(payload, 3),
            Self::Delete => encode_discriminant(payload, 4),
            Self::Connect => encode_discriminant(payload, 5),
            Self::Options => encode_discriminant(payload, 6),
            Self::Trace => encode_discriminant(payload, 7),
            Self::Patch => encode_discriminant(payload, 8),
            Self::Other(s) => {
                encode_discriminant(&mut payload, 9)?;
                s.encode_sync(payload)
            }
        }
    }
}

#[async_trait]
impl Receive for Method {
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        let (discriminant, payload) = receive_discriminant(payload, rx).await?;
        match discriminant {
            0 => Ok((Self::Get, payload)),
            1 => Ok((Self::Head, payload)),
            2 => Ok((Self::Post, payload)),
            3 => Ok((Self::Put, payload)),
            4 => Ok((Self::Delete, payload)),
            5 => Ok((Self::Connect, payload)),
            6 => Ok((Self::Options, payload)),
            7 => Ok((Self::Trace, payload)),
            8 => Ok((Self::Patch, payload)),
            9 => {
                let (s, payload) = Receive::receive_sync(payload, rx).await?;
                Ok((Self::Other(s), payload))
            }
            _ => bail!("unknown discriminant `{discriminant}`"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Scheme {
    HTTP,
    HTTPS,
    Other(String),
}

impl EncodeSync for Scheme {
    #[instrument(level = "trace", skip_all)]
    fn encode_sync(self, mut payload: impl BufMut) -> anyhow::Result<()> {
        match self {
            Self::HTTP => encode_discriminant(payload, 0),
            Self::HTTPS => encode_discriminant(payload, 1),
            Self::Other(s) => {
                encode_discriminant(&mut payload, 2)?;
                s.encode_sync(payload)
            }
        }
    }
}

#[async_trait]
impl Receive for Scheme {
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        let (discriminant, payload) = receive_discriminant(payload, rx).await?;
        match discriminant {
            0 => Ok((Self::HTTP, payload)),
            1 => Ok((Self::HTTPS, payload)),
            2 => {
                let (s, payload) = Receive::receive_sync(payload, rx).await?;
                Ok((Self::Other(s), payload))
            }
            _ => bail!("unknown discriminant `{discriminant}`"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DNSErrorPayload {
    pub rcode: Option<String>,
    pub info_code: Option<u16>,
}

impl EncodeSync for DNSErrorPayload {
    #[instrument(level = "trace", skip_all)]
    fn encode_sync(self, mut payload: impl BufMut) -> anyhow::Result<()> {
        let Self { rcode, info_code } = self;
        EncodeSync::encode_sync_option(rcode, &mut payload).context("failed to encode `rcode`")?;
        EncodeSync::encode_sync_option(info_code, payload)
            .context("failed to encode `info_code`")?;
        Ok(())
    }
}

#[async_trait]
impl Receive for DNSErrorPayload {
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        let (rcode, payload) = Receive::receive_sync(payload, rx)
            .await
            .context("failed to receive `rcode`")?;
        let (info_code, payload) = Receive::receive_sync(payload, rx)
            .await
            .context("failed to receive `info_code`")?;
        Ok((Self { rcode, info_code }, payload))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TLSAlertReceivedPayload {
    pub alert_id: Option<u8>,
    pub alert_message: Option<String>,
}

impl EncodeSync for TLSAlertReceivedPayload {
    #[instrument(level = "trace", skip_all)]
    fn encode_sync(self, mut payload: impl BufMut) -> anyhow::Result<()> {
        let Self {
            alert_id,
            alert_message,
        } = self;
        EncodeSync::encode_sync_option(alert_id, &mut payload)
            .context("failed to encode `alert_id`")?;
        EncodeSync::encode_sync_option(alert_message, payload)
            .context("failed to encode `alert_message`")?;
        Ok(())
    }
}

#[async_trait]
impl Receive for TLSAlertReceivedPayload {
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        let (alert_id, payload) = Receive::receive_sync(payload, rx)
            .await
            .context("failed to receive `alert_id`")?;
        let (alert_message, payload) = Receive::receive_sync(payload, rx)
            .await
            .context("failed to receive `alert_message`")?;
        Ok((
            Self {
                alert_id,
                alert_message,
            },
            payload,
        ))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldSizePayload {
    pub field_name: Option<String>,
    pub field_size: Option<u32>,
}

impl EncodeSync for FieldSizePayload {
    #[instrument(level = "trace", skip_all)]
    fn encode_sync(self, mut payload: impl BufMut) -> anyhow::Result<()> {
        let Self {
            field_name,
            field_size,
        } = self;
        EncodeSync::encode_sync_option(field_name, &mut payload)
            .context("failed to encode `field_name`")?;
        EncodeSync::encode_sync_option(field_size, payload)
            .context("failed to encode `field_size`")?;
        Ok(())
    }
}

#[async_trait]
impl Receive for FieldSizePayload {
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        let (field_name, payload) = Receive::receive_sync(payload, rx)
            .await
            .context("failed to receive `field_name`")?;
        let (field_size, payload) = Receive::receive_sync(payload, rx)
            .await
            .context("failed to receive `field_size`")?;
        Ok((
            Self {
                field_name,
                field_size,
            },
            payload,
        ))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ErrorCode {
    DnsTimeout,
    DnsError(DNSErrorPayload),
    DestinationNotFound,
    DestinationUnavailable,
    DestinationIpProhibited,
    DestinationIpUnroutable,
    ConnectionRefused,
    ConnectionTerminated,
    ConnectionTimeout,
    ConnectionReadTimeout,
    ConnectionWriteTimeout,
    ConnectionLimitReached,
    TlsProtocolError,
    TlsCertificateError,
    TlsAlertReceived(TLSAlertReceivedPayload),
    HttpRequestDenied,
    HttpRequestLengthRequired,
    HttpRequestBodySize(Option<u64>),
    HttpRequestMethodInvalid,
    HttpRequestUriInvalid,
    HttpRequestUriTooLong,
    HttpRequestHeaderSectionSize(Option<u32>),
    HttpRequestHeaderSize(Option<FieldSizePayload>),
    HttpRequestTrailerSectionSize(Option<u32>),
    HttpRequestTrailerSize(FieldSizePayload),
    HttpResponseIncomplete,
    HttpResponseHeaderSectionSize(Option<u32>),
    HttpResponseHeaderSize(FieldSizePayload),
    HttpResponseBodySize(Option<u64>),
    HttpResponseTrailerSectionSize(Option<u32>),
    HttpResponseTrailerSize(Option<u32>),
    HttpResponseTransferCoding(Option<String>),
    HttpResponseContentCoding(Option<String>),
    HttpResponseTimeout,
    HttpUpgradeFailed,
    HttpProtocolError,
    LoopDetected,
    ConfigurationError,
    InternalError(Option<String>),
}

impl EncodeSync for ErrorCode {
    #[instrument(level = "trace", skip_all)]
    fn encode_sync(self, mut payload: impl BufMut) -> anyhow::Result<()> {
        match self {
            Self::DnsTimeout => encode_discriminant(payload, 0),
            Self::DnsError(v) => {
                encode_discriminant(&mut payload, 1)?;
                v.encode_sync(payload)
            }
            Self::DestinationNotFound => encode_discriminant(payload, 2),
            Self::DestinationUnavailable => encode_discriminant(payload, 3),
            Self::DestinationIpProhibited => encode_discriminant(payload, 4),
            Self::DestinationIpUnroutable => encode_discriminant(payload, 5),
            Self::ConnectionRefused => encode_discriminant(payload, 6),
            Self::ConnectionTerminated => encode_discriminant(payload, 7),
            Self::ConnectionTimeout => encode_discriminant(payload, 8),
            Self::ConnectionReadTimeout => encode_discriminant(payload, 9),
            Self::ConnectionWriteTimeout => encode_discriminant(payload, 10),
            Self::ConnectionLimitReached => encode_discriminant(payload, 11),
            Self::TlsProtocolError => encode_discriminant(payload, 12),
            Self::TlsCertificateError => encode_discriminant(payload, 13),
            Self::TlsAlertReceived(v) => {
                encode_discriminant(&mut payload, 14)?;
                v.encode_sync(payload)
            }
            Self::HttpRequestDenied => encode_discriminant(&mut payload, 15),
            Self::HttpRequestLengthRequired => encode_discriminant(&mut payload, 16),
            Self::HttpRequestBodySize(v) => {
                encode_discriminant(&mut payload, 17)?;
                EncodeSync::encode_sync_option(v, payload)
            }
            Self::HttpRequestMethodInvalid => encode_discriminant(&mut payload, 18),
            Self::HttpRequestUriInvalid => encode_discriminant(&mut payload, 19),
            Self::HttpRequestUriTooLong => encode_discriminant(&mut payload, 20),
            Self::HttpRequestHeaderSectionSize(v) => {
                encode_discriminant(&mut payload, 21)?;
                EncodeSync::encode_sync_option(v, payload)
            }
            Self::HttpRequestHeaderSize(v) => {
                encode_discriminant(&mut payload, 22)?;
                EncodeSync::encode_sync_option(v, payload)
            }
            Self::HttpRequestTrailerSectionSize(v) => {
                encode_discriminant(&mut payload, 23)?;
                EncodeSync::encode_sync_option(v, payload)
            }
            Self::HttpRequestTrailerSize(v) => {
                encode_discriminant(&mut payload, 24)?;
                v.encode_sync(payload)
            }
            Self::HttpResponseIncomplete => encode_discriminant(&mut payload, 25),
            Self::HttpResponseHeaderSectionSize(v) => {
                encode_discriminant(&mut payload, 26)?;
                EncodeSync::encode_sync_option(v, payload)
            }
            Self::HttpResponseHeaderSize(v) => {
                encode_discriminant(&mut payload, 27)?;
                v.encode_sync(payload)
            }
            Self::HttpResponseBodySize(v) => {
                encode_discriminant(&mut payload, 28)?;
                EncodeSync::encode_sync_option(v, payload)
            }
            Self::HttpResponseTrailerSectionSize(v) => {
                encode_discriminant(&mut payload, 29)?;
                EncodeSync::encode_sync_option(v, payload)
            }
            Self::HttpResponseTrailerSize(v) => {
                encode_discriminant(&mut payload, 30)?;
                EncodeSync::encode_sync_option(v, payload)
            }
            Self::HttpResponseTransferCoding(v) => {
                encode_discriminant(&mut payload, 31)?;
                EncodeSync::encode_sync_option(v, payload)
            }
            Self::HttpResponseContentCoding(v) => {
                encode_discriminant(&mut payload, 32)?;
                EncodeSync::encode_sync_option(v, payload)
            }
            Self::HttpResponseTimeout => encode_discriminant(&mut payload, 33),
            Self::HttpUpgradeFailed => encode_discriminant(payload, 34),
            Self::HttpProtocolError => encode_discriminant(payload, 35),
            Self::LoopDetected => encode_discriminant(payload, 36),
            Self::ConfigurationError => encode_discriminant(payload, 37),
            Self::InternalError(v) => {
                encode_discriminant(&mut payload, 38)?;
                EncodeSync::encode_sync_option(v, payload)
            }
        }
    }
}

#[async_trait]
impl Receive for ErrorCode {
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        let (discriminant, payload) = receive_discriminant(payload, rx).await?;
        match discriminant {
            0 => Ok((Self::DnsTimeout, payload)),
            1 => {
                let (v, payload) = Receive::receive_sync(payload, rx).await?;
                Ok((Self::DnsError(v), payload))
            }
            2 => Ok((Self::DestinationNotFound, payload)),
            3 => Ok((Self::DestinationUnavailable, payload)),
            4 => Ok((Self::DestinationIpProhibited, payload)),
            5 => Ok((Self::DestinationIpUnroutable, payload)),
            6 => Ok((Self::ConnectionRefused, payload)),
            7 => Ok((Self::ConnectionTerminated, payload)),
            8 => Ok((Self::ConnectionTimeout, payload)),
            9 => Ok((Self::ConnectionReadTimeout, payload)),
            10 => Ok((Self::ConnectionWriteTimeout, payload)),
            11 => Ok((Self::ConnectionLimitReached, payload)),
            12 => Ok((Self::TlsProtocolError, payload)),
            13 => Ok((Self::TlsCertificateError, payload)),
            14 => {
                let (v, payload) = Receive::receive_sync(payload, rx).await?;
                Ok((Self::TlsAlertReceived(v), payload))
            }
            15 => Ok((Self::HttpRequestDenied, payload)),
            16 => Ok((Self::HttpRequestLengthRequired, payload)),
            17 => {
                let (v, payload) = Receive::receive_sync(payload, rx).await?;
                Ok((Self::HttpRequestBodySize(v), payload))
            }
            18 => Ok((Self::HttpRequestMethodInvalid, payload)),
            19 => Ok((Self::HttpRequestUriInvalid, payload)),
            20 => Ok((Self::HttpRequestUriTooLong, payload)),
            21 => {
                let (v, payload) = Receive::receive_sync(payload, rx).await?;
                Ok((Self::HttpRequestHeaderSectionSize(v), payload))
            }
            22 => {
                let (v, payload) = Receive::receive_sync(payload, rx).await?;
                Ok((Self::HttpRequestHeaderSize(v), payload))
            }
            23 => {
                let (v, payload) = Receive::receive_sync(payload, rx).await?;
                Ok((Self::HttpRequestTrailerSectionSize(v), payload))
            }
            24 => {
                let (v, payload) = Receive::receive_sync(payload, rx).await?;
                Ok((Self::HttpRequestTrailerSize(v), payload))
            }
            25 => Ok((Self::HttpResponseIncomplete, payload)),
            26 => {
                let (v, payload) = Receive::receive_sync(payload, rx).await?;
                Ok((Self::HttpResponseHeaderSectionSize(v), payload))
            }
            27 => {
                let (v, payload) = Receive::receive_sync(payload, rx).await?;
                Ok((Self::HttpResponseHeaderSize(v), payload))
            }
            28 => {
                let (v, payload) = Receive::receive_sync(payload, rx).await?;
                Ok((Self::HttpResponseBodySize(v), payload))
            }
            29 => {
                let (v, payload) = Receive::receive_sync(payload, rx).await?;
                Ok((Self::HttpResponseTrailerSectionSize(v), payload))
            }
            30 => {
                let (v, payload) = Receive::receive_sync(payload, rx).await?;
                Ok((Self::HttpResponseTrailerSize(v), payload))
            }
            31 => {
                let (v, payload) = Receive::receive_sync(payload, rx).await?;
                Ok((Self::HttpResponseTransferCoding(v), payload))
            }
            32 => {
                let (v, payload) = Receive::receive_sync(payload, rx).await?;
                Ok((Self::HttpResponseContentCoding(v), payload))
            }
            33 => Ok((Self::HttpResponseTimeout, payload)),
            34 => Ok((Self::HttpUpgradeFailed, payload)),
            35 => Ok((Self::HttpProtocolError, payload)),
            36 => Ok((Self::LoopDetected, payload)),
            37 => Ok((Self::ConfigurationError, payload)),
            38 => {
                let (v, payload) = Receive::receive_sync(payload, rx).await?;
                Ok((Self::InternalError(v), payload))
            }
            _ => bail!("unknown discriminant `{discriminant}`"),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RequestOptions {
    pub connect_timeout: Option<Duration>,
    pub first_byte_timeout: Option<Duration>,
    pub between_bytes_timeout: Option<Duration>,
}

impl EncodeSync for RequestOptions {
    #[instrument(level = "trace", skip_all)]
    fn encode_sync(self, mut payload: impl BufMut) -> anyhow::Result<()> {
        let Self {
            connect_timeout,
            first_byte_timeout,
            between_bytes_timeout,
        } = self;
        EncodeSync::encode_sync_option(connect_timeout, &mut payload)
            .context("failed to encode `connect_timeout`")?;
        EncodeSync::encode_sync_option(first_byte_timeout, &mut payload)
            .context("failed to encode `first_byte_timeout`")?;
        EncodeSync::encode_sync_option(between_bytes_timeout, payload)
            .context("failed to encode `between_bytes_timeout`")?;
        Ok(())
    }
}

#[async_trait]
impl Receive for RequestOptions {
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        let (connect_timeout, payload) = Receive::receive_sync(payload, rx)
            .await
            .context("failed to receive `connect_timeout`")?;
        let (first_byte_timeout, payload) = Receive::receive_sync(payload, rx)
            .await
            .context("failed to receive `first_byte_timeout`")?;
        let (between_bytes_timeout, payload) = Receive::receive_sync(payload, rx)
            .await
            .context("failed to receive `between_bytes_timeout`")?;
        Ok((
            Self {
                connect_timeout,
                first_byte_timeout,
                between_bytes_timeout,
            },
            payload,
        ))
    }
}

pub type IncomingRequest = Request<
    Box<dyn Stream<Item = anyhow::Result<StreamItem<Bytes>>> + Send + Unpin>,
    Pin<Box<dyn Future<Output = anyhow::Result<Option<Fields>>> + Send>>,
>;

pub struct Request<Body, Trailers> {
    pub body: Body,
    pub trailers: Trailers,
    pub method: Method,
    pub path_with_query: Option<String>,
    pub scheme: Option<Scheme>,
    pub authority: Option<String>,
    pub headers: Fields,
}

#[async_trait]
impl<Body, Trailers> Encode for Request<Body, Trailers>
where
    Body: Stream<Item = StreamItem<Bytes>> + Send + 'static,
    Trailers: Future<Output = Option<Fields>> + Send + 'static,
{
    async fn encode(
        self,
        mut payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        let Self {
            body,
            trailers,
            method,
            path_with_query,
            scheme,
            authority,
            headers,
        } = self;
        // mark body as pending
        payload.put_u8(0);
        // mark trailers as pending
        payload.put_u8(0);
        method.encode_sync(&mut payload)?;
        EncodeSync::encode_sync_option(path_with_query, &mut payload)?;
        EncodeSync::encode_sync_option(scheme, &mut payload)?;
        EncodeSync::encode_sync_option(authority, &mut payload)?;
        headers.encode(&mut payload).await?;
        // TODO: Optimize, this wrapping should not be necessary
        Ok(Some(AsyncValue::Record(vec![
            Some(AsyncValue::Stream(Box::pin(body.map(|item| match item {
                StreamItem::Element(buf) => Ok(StreamItem::Element(Some(buf.into()))),
                StreamItem::End => Ok(StreamItem::End),
            })))),
            Some(AsyncValue::Future(Box::pin(async {
                let trailers = trailers.await;
                Ok(Some(trailers.map(fields_to_wrpc).into()))
            }))),
        ])))
    }
}

#[async_trait]
impl Subscribe for IncomingRequest {
    #[instrument(level = "trace", skip_all)]
    async fn subscribe<T: Subscriber + Send + Sync>(
        subscriber: &T,
        subject: T::Subject,
    ) -> Result<Option<AsyncSubscription<T::Stream>>, T::SubscribeError> {
        let (body_subscriber, trailers_subscriber) = try_join!(
            subscriber.subscribe(subject.child(Some(0))),
            subscriber.subscribe(subject.child(Some(1)))
        )?;
        Ok(Some(AsyncSubscription::Record(vec![
            Some(AsyncSubscription::Stream {
                subscriber: body_subscriber,
                nested: None,
            }),
            Some(AsyncSubscription::Future {
                subscriber: trailers_subscriber,
                nested: None,
            }),
        ])))
    }
}

#[async_trait]
impl Receive for IncomingRequest {
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        let mut sub = sub
            .map(AsyncSubscription::try_unwrap_record)
            .transpose()
            .context("stream subscription type mismatch")?;
        let (body, payload) = Receive::receive(
            payload,
            rx,
            sub.as_mut()
                .and_then(|sub| sub.get_mut(0))
                .and_then(Option::take),
        )
        .await
        .context("failed to receive `body`")?;
        let (trailers, payload) = Receive::receive(
            payload,
            rx,
            sub.as_mut()
                .and_then(|sub| sub.get_mut(1))
                .and_then(Option::take),
        )
        .await
        .context("failed to receive `trailers`")?;
        let (method, payload) = Receive::receive_sync(payload, rx)
            .await
            .context("failed to receive `method`")?;
        let (path_with_query, payload) = Receive::receive_sync(payload, rx)
            .await
            .context("failed to receive `path_with_query`")?;
        let (scheme, payload) = Receive::receive_sync(payload, rx)
            .await
            .context("failed to receive `scheme`")?;
        let (authority, payload) = Receive::receive_sync(payload, rx)
            .await
            .context("failed to receive `authority`")?;
        let (headers, payload) = Receive::receive_sync(payload, rx)
            .await
            .context("failed to receive `headers`")?;
        Ok((
            Self {
                body,
                trailers,
                method,
                path_with_query,
                scheme,
                authority,
                headers,
            },
            payload,
        ))
    }
}

pub type IncomingResponse = Response<
    Box<dyn Stream<Item = anyhow::Result<StreamItem<Bytes>>> + Send + Unpin>,
    Pin<Box<dyn Future<Output = anyhow::Result<Option<Fields>>> + Send>>,
>;

pub struct Response<Body, Trailers> {
    pub body: Body,
    pub trailers: Trailers,
    pub status: u16,
    pub headers: Fields,
}

#[async_trait]
impl<Body, Trailers> Encode for Response<Body, Trailers>
where
    Body: Stream<Item = StreamItem<Bytes>> + Send + 'static,
    Trailers: Future<Output = Option<Fields>> + Send + 'static,
{
    async fn encode(
        self,
        mut payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        let Self {
            body,
            trailers,
            status,
            headers,
        } = self;
        // mark body as pending
        payload.put_u8(0);
        // mark trailers as pending
        payload.put_u8(0);
        status.encode_sync(&mut payload)?;
        headers.encode(&mut payload).await?;
        // TODO: Optimize, this wrapping should not be necessary
        Ok(Some(AsyncValue::Record(vec![
            Some(AsyncValue::Stream(Box::pin(body.map(|item| match item {
                StreamItem::Element(buf) => Ok(StreamItem::Element(Some(buf.into()))),
                StreamItem::End => Ok(StreamItem::End),
            })))),
            Some(AsyncValue::Future(Box::pin(async {
                let trailers = trailers.await;
                Ok(Some(trailers.map(fields_to_wrpc).into()))
            }))),
        ])))
    }
}

#[async_trait]
impl Subscribe for IncomingResponse {
    #[instrument(level = "trace", skip_all)]
    async fn subscribe<T: Subscriber + Send + Sync>(
        subscriber: &T,
        subject: T::Subject,
    ) -> Result<Option<AsyncSubscription<T::Stream>>, T::SubscribeError> {
        let (body_subscriber, trailers_subscriber) = try_join!(
            subscriber.subscribe(subject.child(Some(0))),
            subscriber.subscribe(subject.child(Some(1)))
        )?;
        Ok(Some(AsyncSubscription::Record(vec![
            Some(AsyncSubscription::Stream {
                subscriber: body_subscriber,
                nested: None,
            }),
            Some(AsyncSubscription::Future {
                subscriber: trailers_subscriber,
                nested: None,
            }),
        ])))
    }
}

#[async_trait]
impl Receive for IncomingResponse {
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        let mut sub = sub
            .map(AsyncSubscription::try_unwrap_record)
            .transpose()
            .context("stream subscription type mismatch")?;
        let (body, payload) = Receive::receive(
            payload,
            rx,
            sub.as_mut()
                .and_then(|sub| sub.get_mut(0))
                .and_then(Option::take),
        )
        .await
        .context("failed to receive `body`")?;
        let (trailers, payload) = Receive::receive(
            payload,
            rx,
            sub.as_mut()
                .and_then(|sub| sub.get_mut(1))
                .and_then(Option::take),
        )
        .await
        .context("failed to receive `trailers`")?;
        let (status, payload) = Receive::receive_sync(payload, rx)
            .await
            .context("failed to receive `status`")?;
        let (headers, payload) = Receive::receive_sync(payload, rx)
            .await
            .context("failed to receive `headers`")?;
        Ok((
            Self {
                body,
                trailers,
                status,
                headers,
            },
            payload,
        ))
    }
}

#[async_trait]
pub trait IncomingHandler: wrpc_transport::Client {
    type HandleInvocationStream;

    async fn invoke_handle<Body, Trailers>(
        &self,
        request: Request<Body, Trailers>,
    ) -> anyhow::Result<(Result<IncomingResponse, ErrorCode>, Self::Transmission)>
    where
        Body: Stream<Item = StreamItem<Bytes>> + Send + 'static,
        Trailers: Future<Output = Option<Fields>> + Send + 'static;

    async fn serve_handle(&self) -> anyhow::Result<Self::HandleInvocationStream>;
}

#[async_trait]
impl<T: wrpc_transport::Client> IncomingHandler for T {
    type HandleInvocationStream = Pin<
        Box<
            dyn Stream<
                    Item = anyhow::Result<(
                        IncomingRequest,
                        T::Subject,
                        <T::Acceptor as Acceptor>::Transmitter,
                    )>,
                > + Send,
        >,
    >;

    async fn invoke_handle<Body, Trailers>(
        &self,
        request: Request<Body, Trailers>,
    ) -> anyhow::Result<(Result<IncomingResponse, ErrorCode>, T::Transmission)>
    where
        Body: Stream<Item = StreamItem<Bytes>> + Send + 'static,
        Trailers: Future<Output = Option<Fields>> + Send + 'static,
    {
        let (res, tx) = self
            .invoke_static("wrpc:http/incoming-handler@0.1.0", "handle", request)
            .await?;
        Ok((res, tx))
    }

    async fn serve_handle(&self) -> anyhow::Result<Self::HandleInvocationStream> {
        self.serve_static("wrpc:http/incoming-handler@0.1.0", "handle")
            .await
    }
}

#[async_trait]
pub trait OutgoingHandler: wrpc_transport::Client {
    type HandleInvocationStream;

    async fn invoke_handle(
        &self,
        request: Request<
            impl Stream<Item = StreamItem<Bytes>> + Send + 'static,
            impl Future<Output = Option<Fields>> + Send + 'static,
        >,
        options: Option<RequestOptions>,
    ) -> anyhow::Result<(Result<IncomingResponse, ErrorCode>, Self::Transmission)>;

    async fn serve_handle(&self) -> anyhow::Result<Self::HandleInvocationStream>;
}

#[async_trait]
impl<T: wrpc_transport::Client> OutgoingHandler for T {
    type HandleInvocationStream = Pin<
        Box<
            dyn Stream<
                    Item = anyhow::Result<(
                        (IncomingRequest, Option<RequestOptions>),
                        T::Subject,
                        <T::Acceptor as Acceptor>::Transmitter,
                    )>,
                > + Send,
        >,
    >;

    async fn invoke_handle(
        &self,
        request: Request<
            impl Stream<Item = StreamItem<Bytes>> + Send + 'static,
            impl Future<Output = Option<Fields>> + Send + 'static,
        >,
        options: Option<RequestOptions>,
    ) -> anyhow::Result<(Result<IncomingResponse, ErrorCode>, T::Transmission)> {
        let (res, tx) = self
            .invoke_static(
                "wrpc:http/outgoing-handler@0.1.0",
                "handle",
                (request, options),
            )
            .await?;
        Ok((res, tx))
    }

    async fn serve_handle(&self) -> anyhow::Result<Self::HandleInvocationStream> {
        self.serve_static("wrpc:http/outgoing-handler@0.1.0", "handle")
            .await
    }
}
