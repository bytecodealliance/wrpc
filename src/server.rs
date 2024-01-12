use core::future::Future;
use core::iter;
use core::pin::Pin;
use core::str::FromStr;
use core::task::Poll;

use std::collections::{hash_map, HashMap};

use anyhow::{bail, ensure, Context as _};
use bytes::{BufMut, Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::spawn;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::trace;

use crate::oneshot_topic;

pub struct Return(oneshot::Receiver<h2::SendStream<Bytes>>);

#[derive(Debug)]
pub struct Returns(oneshot_topic::Receiver<Vec<u32>, h2::SendStream<Bytes>>);

impl Returns {
    pub fn channel(
        resolve: &wit_parser::Resolve,
        ty: &wit_parser::Results,
    ) -> Option<(oneshot_topic::Sender<Vec<u32>, h2::SendStream<Bytes>>, Self)> {
        let mut paths = match ty {
            wit_parser::Results::Named(ty) => {
                type_iter_paths(resolve, ty.iter().map(|(_, ty)| Some(ty)))
            }
            wit_parser::Results::Anon(ty) => type_paths(resolve, ty),
        }?;
        for path in &mut paths {
            path.reverse()
        }
        let (tx, rx) = oneshot_topic::channel(paths);
        Some((tx, Self(rx)))
    }

    pub fn take(&mut self, path: &Vec<u32>) -> Result<Return, oneshot_topic::Error> {
        self.0.take(path).map(Return)
    }
}

#[derive(Debug)]
pub struct Params(oneshot_topic::Receiver<Vec<u32>, h2::RecvStream>);

impl Params {
    pub fn channel(
        resolve: &wit_parser::Resolve,
        ty: &[(String, wit_parser::Type)],
    ) -> Option<(oneshot_topic::Sender<Vec<u32>, h2::RecvStream>, Self)> {
        let mut paths = type_iter_paths(resolve, ty.iter().map(|(_, ty)| Some(ty)))?;
        for path in &mut paths {
            path.reverse()
        }
        let (tx, rx) = oneshot_topic::channel(paths);
        Some((tx, Self(rx)))
    }

    pub fn take(&mut self, path: &Vec<u32>) -> Result<Param, oneshot_topic::Error> {
        self.0.take(path).map(Param)
    }
}

struct Handler<F> {
    kind: wit_parser::FunctionKind,
    params: wit_parser::Params,
    results: wit_parser::Results,

    func: F,
}

pub struct Server {
    resolve: wit_parser::Resolve,
    handlers: HashMap<
        String,
        Handler<
            Box<
                dyn Fn(Params, Returns) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>,
            >,
        >,
    >,
}

pub struct ServerBuilder<'a> {
    resolve: &'a wit_parser::Resolve,
    handlers: HashMap<
        wit_parser::InterfaceId,
        Handler<
            Box<
                dyn Fn(
                    String,
                    Params,
                    Returns,
                ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>,
            >,
        >,
    >,
}

impl<'a> ServerBuilder<'a> {
    pub fn new(resolve: &'a wit_parser::Resolve) -> Self {
        Self {
            resolve,
            handlers: HashMap::default(),
        }
    }

    pub fn handle<Fut: Future<Output = anyhow::Result<()>> + Send>(
        &mut self,
        _interface: wit_parser::InterfaceId,
        _name: String,
        _handler: impl Fn(Params, Returns) -> Fut,
    ) -> &mut Self {
        //self.handlers.insert(
        //    interface,
        //    Box::new(|name, params, returns| handler(name, params, returns)),
        //);
        todo!();
        self
    }

    pub fn build(self, _nats: async_nats::Client) -> Server {
        todo!()
    }
}

impl Server {
    pub fn new(resolve: wit_parser::Resolve) -> Self {
        Self {
            resolve,
            handlers: Default::default(),
        }
    }

    pub fn handle(
        &mut self,
        interface: wit_parser::InterfaceId,
        func: String,
        handler: Box<
            dyn Fn(Params, Returns) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>,
        >,
    ) -> anyhow::Result<()> {
        let wit_parser::Interface { functions, .. } = self
            .resolve
            .interfaces
            .get(interface)
            .context("failed to find interface")?;
        let wit_parser::Function {
            kind,
            params,
            results,
            ..
        } = functions.get(&func).context("failed to get function")?;
        ensure!(self
            .handlers
            .insert(
                format!("wasi:http/incoming-handler@0.2.0-rc-2023-11-10.{func}"), // TODO: Fix
                Handler {
                    kind: kind.clone(),
                    params: params.clone(),
                    results: results.clone(),
                    func: handler,
                }
            )
            .is_none());
        Ok(())
    }
}

#[derive(Debug)]
struct Invocation {
    params: oneshot_topic::Sender<Vec<u32>, h2::RecvStream>,
    returns: oneshot_topic::Sender<Vec<u32>, h2::SendStream<Bytes>>,
    handler: JoinHandle<anyhow::Result<()>>,
}

pub struct ByteStream {
    stream: h2::RecvStream,
    buffer: Option<Bytes>,
}

impl ByteStream {
    pub fn is_end_stream(&self) -> bool {
        self.buffer.is_none() && self.stream.is_end_stream()
    }
}

impl From<h2::RecvStream> for ByteStream {
    fn from(stream: h2::RecvStream) -> Self {
        Self {
            stream,
            buffer: None,
        }
    }
}

impl AsyncRead for ByteStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let mut data = if let Some(buffer) = self.buffer.take() {
            buffer
        } else {
            match Pin::new(&mut self.stream).poll_data(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Ready(Some(Ok(data))) => {
                    self.stream
                        .flow_control()
                        .release_capacity(data.len())
                        .map_err(|err| {
                            std::io::Error::new(std::io::ErrorKind::Other, err.to_string())
                        })?;
                    data
                }
                Poll::Ready(Some(Err(err))) => {
                    return Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        err.to_string(),
                    )))
                }
            }
        };
        let cap = buf.capacity();
        if data.len() > cap {
            self.buffer = Some(data.split_off(cap));
        }
        buf.put(data);
        Poll::Ready(Ok(()))
    }
}

pub struct Param(oneshot::Receiver<h2::RecvStream>);

impl Param {
    async fn into_stream(self) -> anyhow::Result<h2::RecvStream> {
        self.0.await.ok().context("failed to acquire stream")
    }

    pub async fn into_byte_stream(self) -> anyhow::Result<ByteStream> {
        self.into_stream().await.map(Into::into)
    }

    pub async fn into_u8(self) -> anyhow::Result<u8> {
        let mut stream = self.into_byte_stream().await?;
        let val = stream.read_u8().await.context("failed to read u8")?;
        ensure!(stream.is_end_stream());
        Ok(val)
    }

    pub async fn into_u16(self) -> anyhow::Result<u16> {
        let mut stream = self.into_byte_stream().await?;
        let val = stream.read_u16_le().await.context("failed to read u16")?;
        ensure!(stream.is_end_stream());
        Ok(val)
    }

    pub async fn into_u32(self) -> anyhow::Result<u32> {
        let mut stream = self.into_byte_stream().await?;
        let val = stream.read_u32_le().await.context("failed to read u32")?;
        ensure!(stream.is_end_stream());
        Ok(val)
    }

    pub async fn into_u64(self) -> anyhow::Result<u64> {
        let mut stream = self.into_byte_stream().await?;
        let val = stream.read_u64_le().await.context("failed to read u64")?;
        ensure!(stream.is_end_stream());
        Ok(val)
    }

    pub async fn into_u128(self) -> anyhow::Result<u128> {
        let mut stream = self.into_byte_stream().await?;
        let val = stream.read_u128_le().await.context("failed to read u128")?;
        ensure!(stream.is_end_stream());
        Ok(val)
    }

    pub async fn into_i8(self) -> anyhow::Result<i8> {
        let mut stream = self.into_byte_stream().await?;
        let val = stream.read_i8().await.context("failed to read u8")?;
        ensure!(stream.is_end_stream());
        Ok(val)
    }

    pub async fn into_i16(self) -> anyhow::Result<i16> {
        let mut stream = self.into_byte_stream().await?;
        let val = stream.read_i16_le().await.context("failed to read u16")?;
        ensure!(stream.is_end_stream());
        Ok(val)
    }

    pub async fn into_i32(self) -> anyhow::Result<i32> {
        let mut stream = self.into_byte_stream().await?;
        let val = stream.read_i32_le().await.context("failed to read u32")?;
        ensure!(stream.is_end_stream());
        Ok(val)
    }

    pub async fn into_i64(self) -> anyhow::Result<i64> {
        let mut stream = self.into_byte_stream().await?;
        let val = stream.read_i64_le().await.context("failed to read u64")?;
        ensure!(stream.is_end_stream());
        Ok(val)
    }

    pub async fn into_i128(self) -> anyhow::Result<i128> {
        let mut stream = self.into_byte_stream().await?;
        let val = stream.read_i128_le().await.context("failed to read u128")?;
        ensure!(stream.is_end_stream());
        Ok(val)
    }

    pub async fn into_bool(self) -> anyhow::Result<bool> {
        let mut stream = self.into_stream().await?;
        let data = stream
            .data()
            .await
            .transpose()
            .context("failed to get data frame")?;
        if let Some(ref data) = data {
            stream
                .flow_control()
                .release_capacity(data.len())
                .context("failed to release capacity")?;
        }
        ensure!(stream.is_end_stream());
        match data.as_deref() {
            None | Some([]) | Some([0]) => Ok(false),
            Some([1]) => Ok(true),
            _ => bail!("invalid data"),
        }
    }

    pub async fn into_string(self) -> anyhow::Result<String> {
        let mut stream = self.into_byte_stream().await?;
        let mut val = String::new();
        stream
            .read_to_string(&mut val)
            .await
            .context("failed to read string")?;
        ensure!(stream.is_end_stream());
        Ok(val)
    }

    pub async fn into_bytes(self) -> anyhow::Result<Bytes> {
        let mut stream = self.into_stream().await?;
        let mut buf = BytesMut::new();
        while let Some(data) = stream.data().await {
            let data = data.context("failed to get data frame")?;
            stream
                .flow_control()
                .release_capacity(data.len())
                .context("failed to release capacity")?;
            buf.put(data);
        }
        ensure!(stream.is_end_stream());
        Ok(buf.into())
    }
}

fn type_iter_paths<'a>(
    resolve: &wit_parser::Resolve,
    ty: impl IntoIterator<Item = Option<&'a wit_parser::Type>>,
) -> Option<Vec<Vec<u32>>> {
    iter::zip(0.., ty).fold(Some(Vec::default()), |paths, (i, ty)| {
        let mut paths = paths?;
        if let Some(ty) = ty {
            for mut path in type_paths(resolve, ty)? {
                path.push(i);
                paths.push(path);
            }
        } else {
            paths.push(vec![i])
        }
        Some(paths)
    })
}

fn field_paths(resolve: &wit_parser::Resolve, ty: &[wit_parser::Field]) -> Option<Vec<Vec<u32>>> {
    type_iter_paths(
        resolve,
        ty.iter().map(|wit_parser::Field { ty, .. }| Some(ty)),
    )
}

fn typedef_paths(
    resolve: &wit_parser::Resolve,
    wit_parser::TypeDef { kind, .. }: &wit_parser::TypeDef,
) -> Option<Vec<Vec<u32>>> {
    match kind {
        wit_parser::TypeDefKind::Record(wit_parser::Record { fields }) => {
            field_paths(resolve, fields)
        }
        wit_parser::TypeDefKind::Resource => Some(vec![]),
        wit_parser::TypeDefKind::Handle(
            wit_parser::Handle::Own(ty) | wit_parser::Handle::Borrow(ty),
        ) => {
            let ty = resolve.types.get(*ty)?;
            typedef_paths(resolve, ty)
        }
        wit_parser::TypeDefKind::Flags(wit_parser::Flags { flags }) => flags
            .len()
            .try_into()
            .map(|n| (0..n).map(|i| vec![i]).collect())
            .ok(),
        wit_parser::TypeDefKind::Tuple(wit_parser::Tuple { types }) => {
            type_iter_paths(resolve, types.iter().map(Some))
        }
        wit_parser::TypeDefKind::Variant(wit_parser::Variant { cases }) => type_iter_paths(
            resolve,
            cases
                .iter()
                .map(|wit_parser::Case { ty, .. }| ty.as_ref()),
        ),
        wit_parser::TypeDefKind::Enum(wit_parser::Enum { cases }) => cases
            .len()
            .try_into()
            .map(|n| (0..n).map(|i| vec![i]).collect())
            .ok(),
        wit_parser::TypeDefKind::Option(ty) => type_iter_paths(resolve, [None, Some(ty)]),
        wit_parser::TypeDefKind::Result(wit_parser::Result_ { ok, err }) => {
            type_iter_paths(resolve, [ok.as_ref(), err.as_ref()])
        }
        // TODO: Support structured uploads for lists, futures and streams
        wit_parser::TypeDefKind::List(_)
        | wit_parser::TypeDefKind::Future(_)
        | wit_parser::TypeDefKind::Stream(_) => Some(vec![]),
        wit_parser::TypeDefKind::Type(ty) => type_paths(resolve, ty),
        wit_parser::TypeDefKind::Unknown => None,
    }
}

fn type_paths(resolve: &wit_parser::Resolve, ty: &wit_parser::Type) -> Option<Vec<Vec<u32>>> {
    match ty {
        wit_parser::Type::Bool
        | wit_parser::Type::U8
        | wit_parser::Type::U16
        | wit_parser::Type::U32
        | wit_parser::Type::U64
        | wit_parser::Type::S8
        | wit_parser::Type::S16
        | wit_parser::Type::S32
        | wit_parser::Type::S64
        | wit_parser::Type::Float32
        | wit_parser::Type::Float64
        | wit_parser::Type::Char
        | wit_parser::Type::String => Some(vec![]),
        wit_parser::Type::Id(ty) => {
            let ty = resolve.types.get(*ty)?;
            typedef_paths(resolve, ty)
        }
    }
}

fn send_code(
    mut stream: h2::server::SendResponse<Bytes>,
    code: http::StatusCode,
) -> anyhow::Result<()> {
    let res = http::Response::builder()
        .status(code)
        .body(())
        .context("failed to build response")?;
    stream
        .send_response(res, true)
        .context("failed to send response")?;
    Ok(())
}

fn send_error(
    mut stream: h2::server::SendResponse<Bytes>,
    code: http::StatusCode,
    body: Bytes,
) -> anyhow::Result<()> {
    let res = http::Response::builder()
        .status(code)
        .body(())
        .context("failed to build response")?;
    let mut res = stream
        .send_response(res, false)
        .context("failed to send response")?;
    res.reserve_capacity(body.len());
    res.send_data(body, true)?;
    Ok(())
}

enum Command<'a> {
    Invoke { id: &'a str, func: &'a str },
    SetParam { id: &'a str, path: Vec<u32> },
    TakeReturn { id: &'a str, path: Vec<u32> },
}

impl<'a> TryFrom<&'a http::request::Parts> for Command<'a> {
    type Error = http::StatusCode;

    fn try_from(
        http::request::Parts {
            method,
            uri,
            headers,
            ..
        }: &'a http::request::Parts,
    ) -> Result<Command<'a>, http::StatusCode> {
        let (head, tail) = uri
            .path()
            .split_once('/')
            .ok_or(http::StatusCode::BAD_REQUEST)?;
        match head {
            "invoke" if method == http::Method::POST => Ok(Self::Invoke {
                id: headers
                    .get("wrpc-invocation-id")
                    .map(|id| id.to_str().or(Err(http::StatusCode::BAD_REQUEST)))
                    .transpose()?
                    .unwrap_or("0"),
                func: tail,
            }),
            "invoke" => Err(http::StatusCode::METHOD_NOT_ALLOWED),
            "invocation" => {
                let (id, tail) = tail.split_once('/').ok_or(http::StatusCode::BAD_REQUEST)?;
                let (selector, tail) = tail.split_once('/').ok_or(http::StatusCode::BAD_REQUEST)?;
                let path = tail
                    .split('/')
                    .map(u32::from_str)
                    .collect::<Result<_, _>>()
                    .or(Err(http::StatusCode::BAD_REQUEST))?;
                //.or(Err(http::StatusCode::BAD_REQUEST))?;
                match selector {
                    "params" if method == http::Method::POST => Ok(Self::SetParam { id, path }),
                    "returns" if method == http::Method::DELETE => {
                        Ok(Self::TakeReturn { id, path })
                    }
                    "params" | "returns" => Err(http::StatusCode::METHOD_NOT_ALLOWED),
                    _ => Err(http::StatusCode::BAD_REQUEST),
                }
            }
            _ => Err(http::StatusCode::BAD_REQUEST),
        }
    }
}

impl Server {
    pub fn builder(resolve: &wit_parser::Resolve) -> ServerBuilder<'_> {
        ServerBuilder::new(resolve)
    }

    pub async fn serve(&self, stream: impl AsyncRead + AsyncWrite + Unpin) -> anyhow::Result<()> {
        let mut h2 = h2::server::handshake(stream)
            .await
            .context("failed to perform HTTP/2 handshake")?;
        trace!("performed HTTP/2 handshake");
        let mut invocations: HashMap<String, Invocation> = HashMap::default();
        while let Some(req) = h2.accept().await {
            let (req, response) = req.context("failed to accept HTTP/2 request")?;
            trace!(?req, "accepted HTTP/2 request");
            let (parts, body) = req.into_parts();
            match Command::try_from(&parts) {
                Ok(Command::Invoke { id, func }) => {
                    let Some(Handler {
                        kind: _,
                        params,
                        results,
                        func,
                    }) = self.handlers.get(func)
                    else {
                        send_code(response, http::StatusCode::NOT_FOUND)?;
                        continue;
                    };
                    let Some((params_tx, params_rx)) = Params::channel(&self.resolve, params)
                    else {
                        send_code(response, http::StatusCode::INTERNAL_SERVER_ERROR)?;
                        continue;
                    };
                    let Some((returns_tx, returns_rx)) = Returns::channel(&self.resolve, results)
                    else {
                        send_code(response, http::StatusCode::INTERNAL_SERVER_ERROR)?;
                        continue;
                    };
                    let hash_map::Entry::Vacant(entry) = invocations.entry(id.to_string()) else {
                        send_error(
                            response,
                            http::StatusCode::NOT_FOUND,
                            "unknown function".into(),
                        )?;
                        continue;
                    };
                    entry.insert(Invocation {
                        params: params_tx,
                        returns: returns_tx,
                        handler: spawn((func)(params_rx, returns_rx)),
                    });
                }
                Ok(Command::SetParam { id, path }) => {
                    let Some(Invocation { params, .. }) = invocations.get_mut(id) else {
                        send_code(response, http::StatusCode::BAD_REQUEST)?;
                        continue;
                    };
                    match params.send(&path, body) {
                        Ok(()) => {
                            send_code(response, http::StatusCode::OK)?;
                            continue;
                        }
                        Err(
                            oneshot_topic::Error::Duplicate | oneshot_topic::Error::UnknownTopic,
                        ) => {
                            send_code(response, http::StatusCode::BAD_REQUEST)?;
                            continue;
                        }
                        Err(oneshot_topic::Error::Deadlock) => {
                            send_code(response, http::StatusCode::INTERNAL_SERVER_ERROR)?;
                            continue;
                        }
                    }
                }
                Ok(Command::TakeReturn { id: _, path: _ }) => todo!(),
                Err(_code) => {
                    send_code(response, http::StatusCode::BAD_REQUEST)?;
                    continue;
                }
            };
        }
        anyhow::Ok(())
    }
}
