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
    IncomingInputStream, Receive, Subject as _, Subscribe, Subscriber, Value,
};

pub type Fields = Vec<(String, Vec<Bytes>)>;

pub type IncomingFields = Pin<Box<dyn Future<Output = anyhow::Result<Option<Fields>>> + Send>>;

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

#[cfg(feature = "http")]
pub fn try_fields_to_header_map(fields: Fields) -> anyhow::Result<http::HeaderMap> {
    let mut headers = http::HeaderMap::new();
    for (name, values) in fields {
        let name: http::HeaderName = name.parse().context("failed to parse header name")?;
        let http::header::Entry::Vacant(entry) = headers.entry(name) else {
            bail!("duplicate header entry");
        };
        let Some((first, values)) = values.split_first() else {
            continue;
        };
        let first = first
            .as_ref()
            .try_into()
            .context("failed to construct header value")?;
        let mut entry = entry.insert_entry(first);
        for value in values {
            let value = value
                .as_ref()
                .try_into()
                .context("failed to construct header value")?;
            entry.append(value);
        }
    }
    Ok(headers)
}

#[cfg(feature = "http")]
pub fn try_header_map_to_fields(headers: http::HeaderMap) -> anyhow::Result<Fields> {
    let headers_len = headers.keys_len();
    headers
        .into_iter()
        .try_fold(
            Vec::with_capacity(headers_len),
            |mut headers, (name, value)| {
                if let Some(name) = name {
                    headers.push((name.to_string(), vec![value.as_bytes().to_vec().into()]));
                } else {
                    let (_, ref mut values) = headers
                        .last_mut()
                        .context("header name missing and fields are empty")?;
                    values.push(value.as_bytes().to_vec().into());
                }
                anyhow::Ok(headers)
            },
        )
        .context("failed to construct fields")
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

#[cfg(feature = "http")]
impl From<&http::Method> for Method {
    fn from(method: &http::Method) -> Self {
        match method.as_str() {
            "GET" => Self::Get,
            "HEAD" => Self::Head,
            "POST" => Self::Post,
            "PUT" => Self::Put,
            "DELETE" => Self::Delete,
            "CONNECT" => Self::Connect,
            "OPTIONS" => Self::Options,
            "TRACE" => Self::Trace,
            "PATCH" => Self::Patch,
            _ => Self::Other(method.to_string()),
        }
    }
}

#[cfg(feature = "http")]
impl TryFrom<&Method> for http::method::Method {
    type Error = http::method::InvalidMethod;

    fn try_from(method: &Method) -> Result<Self, Self::Error> {
        match method {
            Method::Get => Ok(Self::GET),
            Method::Head => Ok(Self::HEAD),
            Method::Post => Ok(Self::POST),
            Method::Put => Ok(Self::PUT),
            Method::Delete => Ok(Self::DELETE),
            Method::Connect => Ok(Self::CONNECT),
            Method::Options => Ok(Self::OPTIONS),
            Method::Trace => Ok(Self::TRACE),
            Method::Patch => Ok(Self::PATCH),
            Method::Other(method) => method.parse(),
        }
    }
}

impl Subscribe for Method {}

#[async_trait]
impl Encode for Method {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        mut payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        match self {
            Self::Get => {
                encode_discriminant(payload, 0)?;
                Ok(None)
            }
            Self::Head => {
                encode_discriminant(payload, 1)?;
                Ok(None)
            }
            Self::Post => {
                encode_discriminant(payload, 2)?;
                Ok(None)
            }
            Self::Put => {
                encode_discriminant(payload, 3)?;
                Ok(None)
            }
            Self::Delete => {
                encode_discriminant(payload, 4)?;
                Ok(None)
            }
            Self::Connect => {
                encode_discriminant(payload, 5)?;
                Ok(None)
            }
            Self::Options => {
                encode_discriminant(payload, 6)?;
                Ok(None)
            }
            Self::Trace => {
                encode_discriminant(payload, 7)?;
                Ok(None)
            }
            Self::Patch => {
                encode_discriminant(payload, 8)?;
                Ok(None)
            }
            Self::Other(s) => {
                encode_discriminant(&mut payload, 9)?;
                s.encode(payload).await
            }
        }
    }
}

#[async_trait]
impl<'a> Receive<'a> for Method {
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        mut rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
    {
        let (discriminant, payload) = receive_discriminant(payload, &mut rx).await?;
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

#[cfg(feature = "http")]
impl From<&http::uri::Scheme> for Scheme {
    fn from(scheme: &http::uri::Scheme) -> Self {
        match scheme.as_str() {
            "http" => Self::HTTP,
            "https" => Self::HTTPS,
            _ => Self::Other(scheme.to_string()),
        }
    }
}

#[cfg(feature = "http")]
impl TryFrom<&Scheme> for http::uri::Scheme {
    type Error = http::uri::InvalidUri;

    fn try_from(scheme: &Scheme) -> Result<Self, Self::Error> {
        match scheme {
            Scheme::HTTP => Ok(Self::HTTP),
            Scheme::HTTPS => Ok(Self::HTTPS),
            Scheme::Other(scheme) => scheme.parse(),
        }
    }
}

impl Subscribe for Scheme {}

#[async_trait]
impl Encode for Scheme {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        mut payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        match self {
            Self::HTTP => {
                encode_discriminant(payload, 0)?;
                Ok(None)
            }
            Self::HTTPS => {
                encode_discriminant(payload, 1)?;
                Ok(None)
            }
            Self::Other(s) => {
                encode_discriminant(&mut payload, 2)?;
                s.encode(payload).await
            }
        }
    }
}

#[async_trait]
impl<'a> Receive<'a> for Scheme {
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        mut rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
    {
        let (discriminant, payload) = receive_discriminant(payload, &mut rx).await?;
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
pub struct DnsErrorPayload {
    pub rcode: Option<String>,
    pub info_code: Option<u16>,
}

#[cfg(feature = "wasmtime-wasi-http")]
impl From<wasmtime_wasi_http::bindings::http::types::DnsErrorPayload> for DnsErrorPayload {
    fn from(
        wasmtime_wasi_http::bindings::http::types::DnsErrorPayload { rcode, info_code }: wasmtime_wasi_http::bindings::http::types::DnsErrorPayload,
    ) -> Self {
        Self { rcode, info_code }
    }
}

#[cfg(feature = "wasmtime-wasi-http")]
impl From<DnsErrorPayload> for wasmtime_wasi_http::bindings::http::types::DnsErrorPayload {
    fn from(DnsErrorPayload { rcode, info_code }: DnsErrorPayload) -> Self {
        Self { rcode, info_code }
    }
}

impl Subscribe for DnsErrorPayload {}

#[async_trait]
impl Encode for DnsErrorPayload {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        let Self { rcode, info_code } = self;
        rcode
            .encode(payload)
            .await
            .context("failed to encode `rcode`")?;
        info_code
            .encode(payload)
            .await
            .context("failed to encode `info_code`")?;
        Ok(None)
    }
}

#[async_trait]
impl<'a> Receive<'a> for DnsErrorPayload {
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
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
pub struct TlsAlertReceivedPayload {
    pub alert_id: Option<u8>,
    pub alert_message: Option<String>,
}

#[cfg(feature = "wasmtime-wasi-http")]
impl From<wasmtime_wasi_http::bindings::http::types::TlsAlertReceivedPayload>
    for TlsAlertReceivedPayload
{
    fn from(
        wasmtime_wasi_http::bindings::http::types::TlsAlertReceivedPayload {
            alert_id,
            alert_message,
        }: wasmtime_wasi_http::bindings::http::types::TlsAlertReceivedPayload,
    ) -> Self {
        Self {
            alert_id,
            alert_message,
        }
    }
}

#[cfg(feature = "wasmtime-wasi-http")]
impl From<TlsAlertReceivedPayload>
    for wasmtime_wasi_http::bindings::http::types::TlsAlertReceivedPayload
{
    fn from(
        TlsAlertReceivedPayload {
            alert_id,
            alert_message,
        }: TlsAlertReceivedPayload,
    ) -> Self {
        Self {
            alert_id,
            alert_message,
        }
    }
}

impl Subscribe for TlsAlertReceivedPayload {}

#[async_trait]
impl Encode for TlsAlertReceivedPayload {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        let Self {
            alert_id,
            alert_message,
        } = self;
        alert_id
            .encode(payload)
            .await
            .context("failed to encode `alert_id`")?;
        alert_message
            .encode(payload)
            .await
            .context("failed to encode `alert_message`")?;
        Ok(None)
    }
}

#[async_trait]
impl<'a> Receive<'a> for TlsAlertReceivedPayload {
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
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

#[cfg(feature = "wasmtime-wasi-http")]
impl From<wasmtime_wasi_http::bindings::http::types::FieldSizePayload> for FieldSizePayload {
    fn from(
        wasmtime_wasi_http::bindings::http::types::FieldSizePayload {
            field_name,
            field_size,
        }: wasmtime_wasi_http::bindings::http::types::FieldSizePayload,
    ) -> Self {
        Self {
            field_name,
            field_size,
        }
    }
}

#[cfg(feature = "wasmtime-wasi-http")]
impl From<FieldSizePayload> for wasmtime_wasi_http::bindings::http::types::FieldSizePayload {
    fn from(
        FieldSizePayload {
            field_name,
            field_size,
        }: FieldSizePayload,
    ) -> Self {
        Self {
            field_name,
            field_size,
        }
    }
}

impl Subscribe for FieldSizePayload {}

#[async_trait]
impl Encode for FieldSizePayload {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        let Self {
            field_name,
            field_size,
        } = self;
        field_name
            .encode(payload)
            .await
            .context("failed to encode `field_name`")?;
        field_size
            .encode(payload)
            .await
            .context("failed to encode `field_size`")?;
        Ok(None)
    }
}

#[async_trait]
impl<'a> Receive<'a> for FieldSizePayload {
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
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
    DnsError(DnsErrorPayload),
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
    TlsAlertReceived(TlsAlertReceivedPayload),
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
    HttpResponseTrailerSize(FieldSizePayload),
    HttpResponseTransferCoding(Option<String>),
    HttpResponseContentCoding(Option<String>),
    HttpResponseTimeout,
    HttpUpgradeFailed,
    HttpProtocolError,
    LoopDetected,
    ConfigurationError,
    InternalError(Option<String>),
}

#[cfg(feature = "wasmtime-wasi-http")]
impl From<wasmtime_wasi_http::bindings::http::types::ErrorCode> for ErrorCode {
    fn from(code: wasmtime_wasi_http::bindings::http::types::ErrorCode) -> Self {
        use wasmtime_wasi_http::bindings::http::types;
        match code {
            types::ErrorCode::DnsTimeout => Self::DnsTimeout,
            types::ErrorCode::DnsError(err) => Self::DnsError(err.into()),
            types::ErrorCode::DestinationNotFound => Self::DestinationNotFound,
            types::ErrorCode::DestinationUnavailable => Self::DestinationUnavailable,
            types::ErrorCode::DestinationIpProhibited => Self::DestinationIpProhibited,
            types::ErrorCode::DestinationIpUnroutable => Self::DestinationIpUnroutable,
            types::ErrorCode::ConnectionRefused => Self::ConnectionRefused,
            types::ErrorCode::ConnectionTerminated => Self::ConnectionTerminated,
            types::ErrorCode::ConnectionTimeout => Self::ConnectionTimeout,
            types::ErrorCode::ConnectionReadTimeout => Self::ConnectionReadTimeout,
            types::ErrorCode::ConnectionWriteTimeout => Self::ConnectionWriteTimeout,
            types::ErrorCode::ConnectionLimitReached => Self::ConnectionLimitReached,
            types::ErrorCode::TlsProtocolError => Self::TlsProtocolError,
            types::ErrorCode::TlsCertificateError => Self::TlsCertificateError,
            types::ErrorCode::TlsAlertReceived(err) => Self::TlsAlertReceived(err.into()),
            types::ErrorCode::HttpRequestDenied => Self::HttpRequestDenied,
            types::ErrorCode::HttpRequestLengthRequired => Self::HttpRequestLengthRequired,
            types::ErrorCode::HttpRequestBodySize(size) => Self::HttpRequestBodySize(size),
            types::ErrorCode::HttpRequestMethodInvalid => Self::HttpRequestMethodInvalid,
            types::ErrorCode::HttpRequestUriInvalid => Self::HttpRequestUriInvalid,
            types::ErrorCode::HttpRequestUriTooLong => Self::HttpRequestUriTooLong,
            types::ErrorCode::HttpRequestHeaderSectionSize(err) => {
                Self::HttpRequestHeaderSectionSize(err)
            }
            types::ErrorCode::HttpRequestHeaderSize(err) => {
                Self::HttpRequestHeaderSize(err.map(Into::into))
            }
            types::ErrorCode::HttpRequestTrailerSectionSize(err) => {
                Self::HttpRequestTrailerSectionSize(err)
            }
            types::ErrorCode::HttpRequestTrailerSize(err) => {
                Self::HttpRequestTrailerSize(err.into())
            }
            types::ErrorCode::HttpResponseIncomplete => Self::HttpResponseIncomplete,
            types::ErrorCode::HttpResponseHeaderSectionSize(err) => {
                Self::HttpResponseHeaderSectionSize(err)
            }
            types::ErrorCode::HttpResponseHeaderSize(err) => {
                Self::HttpResponseHeaderSize(err.into())
            }
            types::ErrorCode::HttpResponseBodySize(err) => Self::HttpResponseBodySize(err),
            types::ErrorCode::HttpResponseTrailerSectionSize(err) => {
                Self::HttpResponseTrailerSectionSize(err)
            }
            types::ErrorCode::HttpResponseTrailerSize(err) => {
                Self::HttpResponseTrailerSize(err.into())
            }
            types::ErrorCode::HttpResponseTransferCoding(err) => {
                Self::HttpResponseTransferCoding(err)
            }
            types::ErrorCode::HttpResponseContentCoding(err) => {
                Self::HttpResponseContentCoding(err)
            }
            types::ErrorCode::HttpResponseTimeout => Self::HttpResponseTimeout,
            types::ErrorCode::HttpUpgradeFailed => Self::HttpUpgradeFailed,
            types::ErrorCode::HttpProtocolError => Self::HttpProtocolError,
            types::ErrorCode::LoopDetected => Self::LoopDetected,
            types::ErrorCode::ConfigurationError => Self::ConfigurationError,
            types::ErrorCode::InternalError(err) => Self::InternalError(err),
        }
    }
}

#[cfg(feature = "wasmtime-wasi-http")]
impl From<ErrorCode> for wasmtime_wasi_http::bindings::http::types::ErrorCode {
    fn from(code: ErrorCode) -> Self {
        match code {
            ErrorCode::DnsTimeout => Self::DnsTimeout,
            ErrorCode::DnsError(err) => Self::DnsError(err.into()),
            ErrorCode::DestinationNotFound => Self::DestinationNotFound,
            ErrorCode::DestinationUnavailable => Self::DestinationUnavailable,
            ErrorCode::DestinationIpProhibited => Self::DestinationIpProhibited,
            ErrorCode::DestinationIpUnroutable => Self::DestinationIpUnroutable,
            ErrorCode::ConnectionRefused => Self::ConnectionRefused,
            ErrorCode::ConnectionTerminated => Self::ConnectionTerminated,
            ErrorCode::ConnectionTimeout => Self::ConnectionTimeout,
            ErrorCode::ConnectionReadTimeout => Self::ConnectionReadTimeout,
            ErrorCode::ConnectionWriteTimeout => Self::ConnectionWriteTimeout,
            ErrorCode::ConnectionLimitReached => Self::ConnectionLimitReached,
            ErrorCode::TlsProtocolError => Self::TlsProtocolError,
            ErrorCode::TlsCertificateError => Self::TlsCertificateError,
            ErrorCode::TlsAlertReceived(err) => Self::TlsAlertReceived(err.into()),
            ErrorCode::HttpRequestDenied => Self::HttpRequestDenied,
            ErrorCode::HttpRequestLengthRequired => Self::HttpRequestLengthRequired,
            ErrorCode::HttpRequestBodySize(size) => Self::HttpRequestBodySize(size),
            ErrorCode::HttpRequestMethodInvalid => Self::HttpRequestMethodInvalid,
            ErrorCode::HttpRequestUriInvalid => Self::HttpRequestUriInvalid,
            ErrorCode::HttpRequestUriTooLong => Self::HttpRequestUriTooLong,
            ErrorCode::HttpRequestHeaderSectionSize(err) => Self::HttpRequestHeaderSectionSize(err),
            ErrorCode::HttpRequestHeaderSize(err) => {
                Self::HttpRequestHeaderSize(err.map(Into::into))
            }
            ErrorCode::HttpRequestTrailerSectionSize(err) => {
                Self::HttpRequestTrailerSectionSize(err)
            }
            ErrorCode::HttpRequestTrailerSize(err) => Self::HttpRequestTrailerSize(err.into()),
            ErrorCode::HttpResponseIncomplete => Self::HttpResponseIncomplete,
            ErrorCode::HttpResponseHeaderSectionSize(err) => {
                Self::HttpResponseHeaderSectionSize(err)
            }
            ErrorCode::HttpResponseHeaderSize(err) => Self::HttpResponseHeaderSize(err.into()),
            ErrorCode::HttpResponseBodySize(err) => Self::HttpResponseBodySize(err),
            ErrorCode::HttpResponseTrailerSectionSize(err) => {
                Self::HttpResponseTrailerSectionSize(err)
            }
            ErrorCode::HttpResponseTrailerSize(err) => Self::HttpResponseTrailerSize(err.into()),
            ErrorCode::HttpResponseTransferCoding(err) => Self::HttpResponseTransferCoding(err),
            ErrorCode::HttpResponseContentCoding(err) => Self::HttpResponseContentCoding(err),
            ErrorCode::HttpResponseTimeout => Self::HttpResponseTimeout,
            ErrorCode::HttpUpgradeFailed => Self::HttpUpgradeFailed,
            ErrorCode::HttpProtocolError => Self::HttpProtocolError,
            ErrorCode::LoopDetected => Self::LoopDetected,
            ErrorCode::ConfigurationError => Self::ConfigurationError,
            ErrorCode::InternalError(err) => Self::InternalError(err),
        }
    }
}

impl Subscribe for ErrorCode {}

#[async_trait]
impl Encode for ErrorCode {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        mut payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        match self {
            Self::DnsTimeout => {
                encode_discriminant(payload, 0)?;
                Ok(None)
            }
            Self::DnsError(v) => {
                encode_discriminant(&mut payload, 1)?;
                v.encode(payload).await
            }
            Self::DestinationNotFound => {
                encode_discriminant(payload, 2)?;
                Ok(None)
            }
            Self::DestinationUnavailable => {
                encode_discriminant(payload, 3)?;
                Ok(None)
            }
            Self::DestinationIpProhibited => {
                encode_discriminant(payload, 4)?;
                Ok(None)
            }
            Self::DestinationIpUnroutable => {
                encode_discriminant(payload, 5)?;
                Ok(None)
            }
            Self::ConnectionRefused => {
                encode_discriminant(payload, 6)?;
                Ok(None)
            }
            Self::ConnectionTerminated => {
                encode_discriminant(payload, 7)?;
                Ok(None)
            }
            Self::ConnectionTimeout => {
                encode_discriminant(payload, 8)?;
                Ok(None)
            }
            Self::ConnectionReadTimeout => {
                encode_discriminant(payload, 9)?;
                Ok(None)
            }
            Self::ConnectionWriteTimeout => {
                encode_discriminant(payload, 10)?;
                Ok(None)
            }
            Self::ConnectionLimitReached => {
                encode_discriminant(payload, 11)?;
                Ok(None)
            }
            Self::TlsProtocolError => {
                encode_discriminant(payload, 12)?;
                Ok(None)
            }
            Self::TlsCertificateError => {
                encode_discriminant(payload, 13)?;
                Ok(None)
            }
            Self::TlsAlertReceived(v) => {
                encode_discriminant(&mut payload, 14)?;
                v.encode(payload).await
            }
            Self::HttpRequestDenied => {
                encode_discriminant(&mut payload, 15)?;
                Ok(None)
            }
            Self::HttpRequestLengthRequired => {
                encode_discriminant(&mut payload, 16)?;
                Ok(None)
            }
            Self::HttpRequestBodySize(v) => {
                encode_discriminant(&mut payload, 17)?;
                v.encode(payload).await
            }
            Self::HttpRequestMethodInvalid => {
                encode_discriminant(&mut payload, 18)?;
                Ok(None)
            }
            Self::HttpRequestUriInvalid => {
                encode_discriminant(&mut payload, 19)?;
                Ok(None)
            }
            Self::HttpRequestUriTooLong => {
                encode_discriminant(&mut payload, 20)?;
                Ok(None)
            }
            Self::HttpRequestHeaderSectionSize(v) => {
                encode_discriminant(&mut payload, 21)?;
                v.encode(payload).await
            }
            Self::HttpRequestHeaderSize(v) => {
                encode_discriminant(&mut payload, 22)?;
                v.encode(payload).await
            }
            Self::HttpRequestTrailerSectionSize(v) => {
                encode_discriminant(&mut payload, 23)?;
                v.encode(payload).await
            }
            Self::HttpRequestTrailerSize(v) => {
                encode_discriminant(&mut payload, 24)?;
                v.encode(payload).await
            }
            Self::HttpResponseIncomplete => {
                encode_discriminant(&mut payload, 25)?;
                Ok(None)
            }
            Self::HttpResponseHeaderSectionSize(v) => {
                encode_discriminant(&mut payload, 26)?;
                v.encode(payload).await
            }
            Self::HttpResponseHeaderSize(v) => {
                encode_discriminant(&mut payload, 27)?;
                v.encode(payload).await
            }
            Self::HttpResponseBodySize(v) => {
                encode_discriminant(&mut payload, 28)?;
                v.encode(payload).await
            }
            Self::HttpResponseTrailerSectionSize(v) => {
                encode_discriminant(&mut payload, 29)?;
                v.encode(payload).await
            }
            Self::HttpResponseTrailerSize(v) => {
                encode_discriminant(&mut payload, 30)?;
                v.encode(payload).await
            }
            Self::HttpResponseTransferCoding(v) => {
                encode_discriminant(&mut payload, 31)?;
                v.encode(payload).await
            }
            Self::HttpResponseContentCoding(v) => {
                encode_discriminant(&mut payload, 32)?;
                v.encode(payload).await
            }
            Self::HttpResponseTimeout => {
                encode_discriminant(&mut payload, 33)?;
                Ok(None)
            }
            Self::HttpUpgradeFailed => {
                encode_discriminant(payload, 34)?;
                Ok(None)
            }
            Self::HttpProtocolError => {
                encode_discriminant(payload, 35)?;
                Ok(None)
            }
            Self::LoopDetected => {
                encode_discriminant(payload, 36)?;
                Ok(None)
            }
            Self::ConfigurationError => {
                encode_discriminant(payload, 37)?;
                Ok(None)
            }
            Self::InternalError(v) => {
                encode_discriminant(&mut payload, 38)?;
                v.encode(payload).await
            }
        }
    }
}

#[async_trait]
impl<'a> Receive<'a> for ErrorCode {
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        mut rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
    {
        let (discriminant, payload) = receive_discriminant(payload, &mut rx).await?;
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

impl Subscribe for RequestOptions {}

#[async_trait]
impl Encode for RequestOptions {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        mut payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        let Self {
            connect_timeout,
            first_byte_timeout,
            between_bytes_timeout,
        } = self;
        connect_timeout
            .encode(&mut payload)
            .await
            .context("failed to encode `connect_timeout`")?;
        first_byte_timeout
            .encode(&mut payload)
            .await
            .context("failed to encode `first_byte_timeout`")?;
        between_bytes_timeout
            .encode(payload)
            .await
            .context("failed to encode `between_bytes_timeout`")?;
        Ok(None)
    }
}

#[async_trait]
impl<'a> Receive<'a> for RequestOptions {
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
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

#[cfg(feature = "http-body")]
pub struct IncomingBody<Body, Trailers> {
    pub body: Body,
    pub trailers: Trailers,
}

#[cfg(feature = "http-body")]
impl<Body, Trailers> http_body::Body for IncomingBody<Body, Trailers>
where
    Body: Stream<Item = anyhow::Result<Bytes>> + Unpin,
    Trailers: Future<Output = anyhow::Result<Option<Fields>>> + Unpin,
{
    type Data = Bytes;
    type Error = anyhow::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        use core::task::Poll;
        use futures::FutureExt as _;

        match self.body.poll_next_unpin(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(Err(err))) => Poll::Ready(Some(Err(err))),
            Poll::Ready(Some(Ok(buf))) => Poll::Ready(Some(Ok(http_body::Frame::data(buf)))),
            Poll::Ready(None) => match self.trailers.poll_unpin(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(Err(err)) => Poll::Ready(Some(Err(err))),
                Poll::Ready(Ok(None)) => Poll::Ready(None),
                Poll::Ready(Ok(Some(trailers))) => {
                    let trailers = try_fields_to_header_map(trailers)
                        .context("failed to convert trailer fields to header map")?;
                    Poll::Ready(Some(Ok(http_body::Frame::trailers(trailers))))
                }
            },
        }
    }
}

#[cfg(feature = "http-body")]
pub enum HttpBodyError<E> {
    InvalidFrame,
    TrailerReceiverClosed,
    HeaderConversion(anyhow::Error),
    Body(E),
}

#[cfg(feature = "http-body")]
impl<E: core::fmt::Debug> core::fmt::Debug for HttpBodyError<E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidFrame => write!(f, "frame is not valid"),
            Self::TrailerReceiverClosed => write!(f, "trailer receiver is closed"),
            Self::HeaderConversion(err) => write!(f, "failed to convert headers: {err:#}"),
            Self::Body(err) => write!(f, "encountered a body error: {err:?}"),
        }
    }
}

#[cfg(feature = "http-body")]
impl<E: core::fmt::Display> core::fmt::Display for HttpBodyError<E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidFrame => write!(f, "frame is not valid"),
            Self::TrailerReceiverClosed => write!(f, "trailer receiver is closed"),
            Self::HeaderConversion(err) => write!(f, "failed to convert headers: {err:#}"),
            Self::Body(err) => write!(f, "encountered a body error: {err}"),
        }
    }
}

#[cfg(feature = "http-body")]
impl<E: core::fmt::Debug + core::fmt::Display> std::error::Error for HttpBodyError<E> {}

#[cfg(feature = "http-body")]
pub fn split_http_body<E>(
    body: impl http_body::Body<Data = Bytes, Error = E>,
) -> (
    impl Stream<Item = Result<Bytes, HttpBodyError<E>>>,
    impl Future<Output = Option<Fields>>,
) {
    let (trailers_tx, mut trailers_rx) = tokio::sync::mpsc::channel(1);
    let body = http_body_util::BodyStream::new(body).filter_map(move |frame| {
        let trailers_tx = trailers_tx.clone();
        async move {
            match frame {
                Ok(frame) => match frame.into_data() {
                    Ok(buf) => Some(Ok(buf)),
                    Err(trailers) => match trailers.into_trailers() {
                        Ok(trailers) => match try_header_map_to_fields(trailers) {
                            Ok(trailers) => {
                                if trailers_tx.send(trailers).await.is_err() {
                                    Some(Err(HttpBodyError::TrailerReceiverClosed))
                                } else {
                                    None
                                }
                            }
                            Err(err) => Some(Err(HttpBodyError::HeaderConversion(err))),
                        },
                        Err(_) => Some(Err(HttpBodyError::InvalidFrame)),
                    },
                },
                Err(err) => Some(Err(HttpBodyError::Body(err))),
            }
        }
    });
    let trailers = async move { trailers_rx.recv().await };
    (body, trailers)
}

#[cfg(feature = "http-body")]
#[instrument(level = "trace", skip_all)]
pub fn split_outgoing_http_body<E>(
    body: impl http_body::Body<Data = Bytes, Error = E>,
) -> (
    impl Stream<Item = Bytes>,
    impl Future<Output = Option<Fields>>,
    impl Stream<Item = HttpBodyError<E>>,
) {
    let (body, trailers) = split_http_body(body);
    let (errors_tx, errors_rx) = tokio::sync::mpsc::channel(1);
    let body = body.filter_map(move |res| {
        let errors_tx = errors_tx.clone();
        async move {
            match res {
                Ok(buf) => Some(buf),
                Err(err) => {
                    if errors_tx.send(err).await.is_err() {
                        tracing::trace!("failed to send body error");
                    }
                    None
                }
            }
        }
    });
    let errors_rx = tokio_stream::wrappers::ReceiverStream::new(errors_rx);
    (body, trailers, errors_rx)
}

pub type IncomingRequest = Request<IncomingInputStream, IncomingFields>;

pub struct Request<Body, Trailers> {
    pub body: Body,
    pub trailers: Trailers,
    pub method: Method,
    pub path_with_query: Option<String>,
    pub scheme: Option<Scheme>,
    pub authority: Option<String>,
    pub headers: Fields,
}

#[cfg(feature = "http-body")]
impl TryFrom<IncomingRequest> for http::Request<IncomingBody<IncomingInputStream, IncomingFields>> {
    type Error = anyhow::Error;

    fn try_from(
        Request {
            body,
            trailers,
            method,
            path_with_query,
            scheme,
            authority,
            headers,
        }: IncomingRequest,
    ) -> Result<Self, Self::Error> {
        let uri = http::Uri::builder();
        let uri = if let Some(path_with_query) = path_with_query {
            uri.path_and_query(path_with_query)
        } else {
            uri.path_and_query("/")
        };
        let uri = if let Some(scheme) = scheme {
            let scheme =
                http::uri::Scheme::try_from(&scheme).context("failed to convert scheme")?;
            uri.scheme(scheme)
        } else {
            uri
        };
        let uri = if let Some(authority) = authority {
            uri.authority(authority)
        } else {
            uri
        };
        let uri = uri.build().context("failed to build URI")?;
        let method = http::method::Method::try_from(&method).context("failed to convert method")?;
        let mut req = http::Request::builder().method(method).uri(uri);
        let req_headers = req
            .headers_mut()
            .context("failed to construct header map")?;
        *req_headers = try_fields_to_header_map(headers)
            .context("failed to convert header fields to header map")?;
        req.body(IncomingBody { body, trailers })
            .context("failed to construct request")
    }
}

#[cfg(feature = "wasmtime-wasi-http")]
impl TryFrom<IncomingRequest> for http::Request<wasmtime_wasi_http::body::HyperIncomingBody> {
    type Error = anyhow::Error;

    fn try_from(req: IncomingRequest) -> Result<Self, Self::Error> {
        use http_body_util::BodyExt as _;
        use wasmtime_wasi_http::body::HyperIncomingBody;

        let req: http::Request<IncomingBody<IncomingInputStream, IncomingFields>> =
            req.try_into()?;
        Ok(req.map(|IncomingBody { body, trailers }| {
            // TODO: Make trailers Sync and remove this
            let trailers = tokio::spawn(trailers);
            HyperIncomingBody::new(
                IncomingBody {
                    body,
                    trailers: Box::pin(async { trailers.await.unwrap() }),
                }
                .map_err(|err| {
                    wasmtime_wasi_http::bindings::http::types::ErrorCode::InternalError(Some(
                        format!("{err:#}"),
                    ))
                }),
            )
        }))
    }
}

/// Attempt converting [`http::Request`] to [`Request`].
/// Values of `path_with_query`, `scheme` and `authority` will be taken from the
/// request URI
#[cfg(feature = "http-body")]
#[instrument(level = "trace", skip_all)]
pub fn try_http_to_outgoing_request<E>(
    request: http::Request<impl http_body::Body<Data = Bytes, Error = E> + Send + 'static>,
) -> anyhow::Result<(
    Request<
        impl Stream<Item = Bytes> + Send + 'static,
        impl Future<Output = Option<Fields>> + Send + 'static,
    >,
    impl Stream<Item = HttpBodyError<E>>,
)>
where
    E: Send + 'static,
{
    let (
        http::request::Parts {
            ref method,
            uri,
            headers,
            ..
        },
        body,
    ) = request.into_parts();
    let headers = try_header_map_to_fields(headers)?;
    let (body, trailers, errors) = split_outgoing_http_body(body);
    Ok((
        Request {
            body,
            trailers,
            method: method.into(),
            path_with_query: uri.path_and_query().map(ToString::to_string),
            scheme: uri.scheme().map(Into::into),
            authority: uri.authority().map(ToString::to_string),
            headers,
        },
        errors,
    ))
}

/// Attempt converting [`wasmtime_wasi_http::types::Request`] to [`Request`].
/// Values of `path_with_query`, `scheme` and `authority` will be taken from the
/// request URI.
#[cfg(feature = "wasmtime-wasi-http")]
#[instrument(level = "trace", skip_all)]
pub fn try_wasmtime_to_outgoing_request(
    wasmtime_wasi_http::types::OutgoingRequest {
        use_tls: _,
        authority,
        request,
        connect_timeout,
        first_byte_timeout,
        between_bytes_timeout,
    }: wasmtime_wasi_http::types::OutgoingRequest,
) -> anyhow::Result<(
    Request<
        impl Stream<Item = Bytes> + Send + 'static,
        impl Future<Output = Option<Fields>> + Send + 'static,
    >,
    RequestOptions,
    impl Stream<Item = HttpBodyError<wasmtime_wasi_http::bindings::http::types::ErrorCode>>,
)> {
    let (
        http::request::Parts {
            ref method,
            uri,
            headers,
            ..
        },
        body,
    ) = request.into_parts();
    let headers = try_header_map_to_fields(headers)?;
    let (body, trailers, errors) = split_outgoing_http_body(body);
    Ok((
        Request {
            body,
            trailers,
            method: method.into(),
            path_with_query: uri.path_and_query().map(ToString::to_string),
            scheme: uri.scheme().map(Into::into),
            authority: Some(authority),
            headers,
        },
        RequestOptions {
            connect_timeout: Some(connect_timeout),
            first_byte_timeout: Some(first_byte_timeout),
            between_bytes_timeout: Some(between_bytes_timeout),
        },
        errors,
    ))
}

#[async_trait]
impl<Body, Trailers> Encode for Request<Body, Trailers>
where
    Body: Stream<Item = Bytes> + Send + 'static,
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
        method.encode(&mut payload).await?;
        path_with_query.encode(&mut payload).await?;
        scheme.encode(&mut payload).await?;
        authority.encode(&mut payload).await?;
        headers.encode(&mut payload).await?;
        // TODO: Optimize, this wrapping should not be necessary
        Ok(Some(AsyncValue::Record(vec![
            Some(AsyncValue::Stream(Box::pin(body.map(|buf| {
                Ok(buf.into_iter().map(Value::U8).map(Some).collect())
            })))),
            Some(AsyncValue::Future(Box::pin(async {
                let trailers = trailers.await;
                Ok(Some(trailers.map(fields_to_wrpc).into()))
            }))),
        ])))
    }
}

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
impl<'a> Receive<'a> for IncomingRequest {
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
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

/// `http` incoming HTTP request wrapper
#[cfg(feature = "http-body")]
pub struct IncomingRequestHttp(
    pub http::Request<IncomingBody<IncomingInputStream, IncomingFields>>,
);

#[cfg(feature = "http-body")]
impl From<IncomingRequestHttp>
    for http::Request<IncomingBody<IncomingInputStream, IncomingFields>>
{
    fn from(IncomingRequestHttp(req): IncomingRequestHttp) -> Self {
        req
    }
}

#[cfg(feature = "http-body")]
#[async_trait]
impl<'a> Receive<'a> for IncomingRequestHttp {
    #[instrument(level = "trace", skip_all)]
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
    {
        // TODO: Optimize by directly receiving `http` values
        let (req, payload) = IncomingRequest::receive(payload, rx, sub)
            .await
            .context("failed to receive request `wrpc:http` request")?;
        let req = req
            .try_into()
            .context("failed to convert `wrpc:http` request to `http`")?;
        Ok((Self(req), payload))
    }
}

#[cfg(feature = "http-body")]
impl Subscribe for IncomingRequestHttp {
    #[instrument(level = "trace", skip_all)]
    async fn subscribe<T: Subscriber + Send + Sync>(
        subscriber: &T,
        subject: T::Subject,
    ) -> Result<Option<AsyncSubscription<T::Stream>>, T::SubscribeError> {
        IncomingRequest::subscribe(subscriber, subject).await
    }
}

/// Wasmtime incoming HTTP request wrapper
#[cfg(feature = "wasmtime-wasi-http")]
pub struct IncomingRequestWasmtime(pub http::Request<wasmtime_wasi_http::body::HyperIncomingBody>);

#[cfg(feature = "wasmtime-wasi-http")]
impl From<IncomingRequestWasmtime> for http::Request<wasmtime_wasi_http::body::HyperIncomingBody> {
    fn from(IncomingRequestWasmtime(req): IncomingRequestWasmtime) -> Self {
        req
    }
}

#[cfg(feature = "wasmtime-wasi-http")]
#[async_trait]
impl<'a> Receive<'a> for IncomingRequestWasmtime {
    #[instrument(level = "trace", skip_all)]
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
    {
        // TODO: Optimize by directly receiving `http` values
        let (req, payload) = IncomingRequest::receive(payload, rx, sub)
            .await
            .context("failed to receive request `wrpc:http` request")?;
        let req = req
            .try_into()
            .context("failed to convert `wrpc:http` request to Wasmtime `wasi:http`")?;
        Ok((Self(req), payload))
    }
}

#[cfg(feature = "wasmtime-wasi-http")]
impl Subscribe for IncomingRequestWasmtime {
    #[instrument(level = "trace", skip_all)]
    async fn subscribe<T: Subscriber + Send + Sync>(
        subscriber: &T,
        subject: T::Subject,
    ) -> Result<Option<AsyncSubscription<T::Stream>>, T::SubscribeError> {
        IncomingRequest::subscribe(subscriber, subject).await
    }
}

pub type IncomingResponse = Response<IncomingInputStream, IncomingFields>;

pub struct Response<Body, Trailers> {
    pub body: Body,
    pub trailers: Trailers,
    pub status: u16,
    pub headers: Fields,
}

#[cfg(feature = "http-body")]
impl TryFrom<IncomingResponse>
    for http::Response<IncomingBody<IncomingInputStream, IncomingFields>>
{
    type Error = anyhow::Error;

    fn try_from(
        Response {
            body,
            trailers,
            status,
            headers,
        }: IncomingResponse,
    ) -> Result<Self, Self::Error> {
        let mut resp = http::Response::builder().status(status);
        let resp_headers = resp
            .headers_mut()
            .context("failed to construct header map")?;
        *resp_headers = try_fields_to_header_map(headers)
            .context("failed to convert header fields to header map")?;
        resp.body(IncomingBody { body, trailers })
            .context("failed to construct response")
    }
}

#[cfg(feature = "wasmtime-wasi-http")]
impl TryFrom<IncomingResponse> for http::Response<wasmtime_wasi_http::body::HyperIncomingBody> {
    type Error = anyhow::Error;

    fn try_from(
        Response {
            body,
            trailers,
            status,
            headers,
        }: IncomingResponse,
    ) -> Result<Self, Self::Error> {
        use http_body_util::BodyExt as _;
        use wasmtime_wasi_http::body::HyperIncomingBody;

        let mut resp = http::Response::builder().status(status);
        let resp_headers = resp
            .headers_mut()
            .context("failed to construct header map")?;
        *resp_headers = try_fields_to_header_map(headers)
            .context("failed to convert header fields to header map")?;
        // TODO: Make trailers Sync and remove this
        let trailers = tokio::spawn(trailers);
        resp.body(HyperIncomingBody::new(
            IncomingBody {
                body,
                trailers: Box::pin(async { trailers.await.unwrap() }),
            }
            .map_err(|err| {
                wasmtime_wasi_http::bindings::http::types::ErrorCode::InternalError(Some(format!(
                    "{err:#}"
                )))
            }),
        ))
        .context("failed to construct response")
    }
}

#[cfg(feature = "http-body")]
#[instrument(level = "trace", skip_all)]
pub fn try_http_to_outgoing_response<E>(
    response: http::Response<impl http_body::Body<Data = Bytes, Error = E> + Send + 'static>,
) -> anyhow::Result<(
    Response<
        impl Stream<Item = Bytes> + Send + 'static,
        impl Future<Output = Option<Fields>> + Send + 'static,
    >,
    impl Stream<Item = HttpBodyError<E>>,
)>
where
    E: Send + 'static,
{
    let (
        http::response::Parts {
            status, headers, ..
        },
        body,
    ) = response.into_parts();
    let headers = try_header_map_to_fields(headers)?;
    let (body, trailers, errors) = split_outgoing_http_body(body);
    Ok((
        Response {
            body,
            trailers,
            status: status.into(),
            headers,
        },
        errors,
    ))
}

#[async_trait]
impl<Body, Trailers> Encode for Response<Body, Trailers>
where
    Body: Stream<Item = Bytes> + Send + 'static,
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
        status.encode(&mut payload).await?;
        headers.encode(&mut payload).await?;
        // TODO: Optimize, this wrapping should not be necessary
        Ok(Some(AsyncValue::Record(vec![
            Some(AsyncValue::Stream(Box::pin(body.map(|buf| {
                Ok(buf.into_iter().map(Value::U8).map(Some).collect())
            })))),
            Some(AsyncValue::Future(Box::pin(async {
                let trailers = trailers.await;
                Ok(Some(trailers.map(fields_to_wrpc).into()))
            }))),
        ])))
    }
}

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
impl<'a> Receive<'a> for IncomingResponse {
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
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

pub trait IncomingHandler: wrpc_transport::Client {
    #[instrument(level = "trace", skip_all)]
    fn invoke_handle<Body, Trailers>(
        &self,
        request: Request<Body, Trailers>,
    ) -> impl Future<
        Output = anyhow::Result<(Result<IncomingResponse, ErrorCode>, Self::Transmission)>,
    > + Send
    where
        Body: Stream<Item = Bytes> + Send + 'static,
        Trailers: Future<Output = Option<Fields>> + Send + 'static,
    {
        self.invoke_static("wrpc:http/incoming-handler@0.1.0", "handle", request)
    }

    #[cfg(feature = "http-body")]
    #[instrument(level = "trace", skip_all)]
    fn invoke_handle_http<E: Send + 'static>(
        &self,
        request: http::Request<impl http_body::Body<Data = Bytes, Error = E> + Send + 'static>,
    ) -> impl Future<
        Output = anyhow::Result<(
            Result<http::Response<IncomingBody<IncomingInputStream, IncomingFields>>, ErrorCode>,
            Self::Transmission,
            impl Stream<Item = HttpBodyError<E>>,
        )>,
    > + Send {
        async {
            let (request, errors) = try_http_to_outgoing_request(request).context(
                "failed to convert incoming `http` request to outgoing `wrpc:http/types.request`",
            )?;
            let (response, tx) = IncomingHandler::invoke_handle(self, request)
                .await
                .context("failed to invoke `wrpc:http/incoming-handler.handle`")?;
            match response {
                Ok(response) => {
                    let response = response.try_into().context(
                "failed to convert incoming `wrpc:http/types.response` to outgoing `http` response",
            )?;
                    Ok((Ok(response), tx, errors))
                }
                Err(code) => Ok((Err(code), tx, errors)),
            }
        }
    }

    #[cfg(feature = "hyper")]
    #[instrument(level = "trace", skip_all)]
    fn invoke_handle_hyper(
        &self,
        request: hyper::Request<hyper::body::Incoming>,
    ) -> impl Future<
        Output = anyhow::Result<(
            Result<
                hyper::Response<IncomingBody<IncomingInputStream, IncomingFields>>,
                anyhow::Error,
            >,
            Self::Transmission,
            impl Stream<Item = HttpBodyError<hyper::Error>>,
        )>,
    > + Send {
        async {
            let (response, tx, errors) = self.invoke_handle_http(request).await?;
            match response {
                Ok(response) => {
                    let (parts, body) = response.into_parts();
                    Ok((Ok(hyper::Response::from_parts(parts, body)), tx, errors))
                }
                Err(code) => Ok((
                    Err(anyhow::anyhow!("handler failed with code `{code:?}`")),
                    tx,
                    errors,
                )),
            }
        }
    }

    #[instrument(level = "trace", skip_all)]
    fn serve_handle(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            Self::InvocationStream<
                Self::Context,
                IncomingRequest,
                <Self::Acceptor as Acceptor>::Transmitter,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:http/incoming-handler@0.1.0", "handle")
    }

    #[cfg(feature = "wasmtime-wasi-http")]
    #[instrument(level = "trace", skip_all)]
    fn serve_handle_wasmtime(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            Self::InvocationStream<
                Self::Context,
                IncomingRequestWasmtime,
                <Self::Acceptor as Acceptor>::Transmitter,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:http/incoming-handler@0.1.0", "handle")
    }
}

impl<T: wrpc_transport::Client> IncomingHandler for T {}

pub trait OutgoingHandler: wrpc_transport::Client {
    #[instrument(level = "trace", skip_all)]
    fn invoke_handle(
        &self,
        request: Request<
            impl Stream<Item = Bytes> + Send + 'static,
            impl Future<Output = Option<Fields>> + Send + 'static,
        >,
        options: Option<RequestOptions>,
    ) -> impl Future<
        Output = anyhow::Result<(Result<IncomingResponse, ErrorCode>, Self::Transmission)>,
    > + Send {
        self.invoke_static(
            "wrpc:http/outgoing-handler@0.1.0",
            "handle",
            (request, options),
        )
    }

    #[cfg(feature = "wasmtime-wasi-http")]
    #[instrument(level = "trace", skip_all)]
    fn invoke_handle_wasmtime(
        &self,
        request: wasmtime_wasi_http::types::OutgoingRequest,
    ) -> impl Future<
        Output = anyhow::Result<(
            Result<
                http::Response<wasmtime_wasi_http::body::HyperIncomingBody>,
                wasmtime_wasi_http::bindings::http::types::ErrorCode,
            >,
            impl Stream<Item = HttpBodyError<wasmtime_wasi_http::bindings::http::types::ErrorCode>>,
            Self::Transmission,
        )>,
    > + Send {
        async {
            let (req, opts, errors) = try_wasmtime_to_outgoing_request(request)?;
            let (resp, tx) = OutgoingHandler::invoke_handle(self, req, Some(opts))
                .await
                .context("failed to invoke `wrpc:http/outgoing-handler.handle`")?;
            match resp {
                Ok(resp) => {
                    let resp = resp.try_into().context(
                        "failed to convert `wrpc:http` response to Wasmtime `wasi:http`",
                    )?;
                    Ok((Ok(resp), errors, tx))
                }
                Err(code) => Ok((Err(code.into()), errors, tx)),
            }
        }
    }

    #[instrument(level = "trace", skip_all)]
    fn serve_handle(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            Self::InvocationStream<
                Self::Context,
                (IncomingRequest, Option<RequestOptions>),
                <Self::Acceptor as Acceptor>::Transmitter,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:http/outgoing-handler@0.1.0", "handle")
    }

    #[cfg(feature = "http-body")]
    #[instrument(level = "trace", skip_all)]
    fn serve_handle_http(
        &self,
    ) -> impl Future<
        Output = anyhow::Result<
            Self::InvocationStream<
                Self::Context,
                (IncomingRequestHttp, Option<RequestOptions>),
                <Self::Acceptor as Acceptor>::Transmitter,
            >,
        >,
    > + Send {
        self.serve_static("wrpc:http/outgoing-handler@0.1.0", "handle")
    }
}

impl<T: wrpc_transport::Client> OutgoingHandler for T {}
