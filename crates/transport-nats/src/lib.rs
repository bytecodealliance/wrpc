use core::future::Future;
use core::iter::zip;
use core::pin::Pin;
use core::str;
use core::task::{Context, Poll};

use std::sync::Arc;

use anyhow::{anyhow, ensure, Context as _};
use async_nats::subject::ToSubject;
use async_nats::{HeaderMap, ServerInfo, StatusCode};
use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use futures::future::{try_join_all, FutureExt as _};
use futures::stream::FuturesUnordered;
use futures::{Stream, StreamExt};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::{spawn, try_join};
use tracing::{instrument, trace};
use wrpc_transport::{AsyncTransmission, Encode, Subject as _, Transmitter as _, PROTOCOL};

#[derive(Debug)]
struct Message {
    pub payload: Bytes,
    pub headers: Option<HeaderMap>,
    pub subject: async_nats::Subject,
    pub reply: Option<async_nats::Subject>,
}

impl TryFrom<async_nats::Message> for Message {
    type Error = std::io::Error;

    #[instrument(level = "trace")]
    fn try_from(msg: async_nats::Message) -> Result<Self, Self::Error> {
        match msg {
            async_nats::Message {
                reply,
                payload,
                headers,
                status: None | Some(StatusCode::OK),
                subject,
                ..
            } => Ok(Self {
                payload,
                headers,
                subject,
                reply,
            }),
            async_nats::Message {
                status: Some(StatusCode::NO_RESPONDERS),
                ..
            } => Err(std::io::ErrorKind::NotConnected.into()),
            async_nats::Message {
                status: Some(StatusCode::TIMEOUT),
                ..
            } => Err(std::io::ErrorKind::TimedOut.into()),
            async_nats::Message {
                status: Some(StatusCode::REQUEST_TERMINATED),
                ..
            } => Err(std::io::ErrorKind::UnexpectedEof.into()),
            async_nats::Message {
                status: Some(code),
                description,
                ..
            } => Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                if let Some(description) = description {
                    format!("received a response with code `{code}` ({description})")
                } else {
                    format!("received a response with code `{code}`")
                },
            )),
        }
    }
}

#[derive(Debug)]
pub struct Request {
    pub payload: Bytes,
    pub headers: Option<HeaderMap>,
    pub subject: async_nats::Subject,
    pub tx: async_nats::Subject,
}

impl TryFrom<Message> for Request {
    type Error = anyhow::Error;

    fn try_from(
        Message {
            payload,
            headers,
            subject,
            reply,
        }: Message,
    ) -> Result<Self, Self::Error> {
        let reply = reply.context("peer did not specify reply subject")?;
        Ok(Self {
            payload,
            headers,
            subject,
            tx: reply,
        })
    }
}

impl TryFrom<async_nats::Message> for Request {
    type Error = anyhow::Error;

    fn try_from(msg: async_nats::Message) -> Result<Self, Self::Error> {
        let msg: Message = msg.try_into()?;
        msg.try_into()
    }
}

pub struct Transmission {
    handle: JoinHandle<anyhow::Result<oneshot::Sender<()>>>,
}

impl Future for Transmission {
    type Output = anyhow::Result<()>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.handle).poll(cx) {
            Poll::Ready(Ok(Ok(_))) => Poll::Ready(Ok(())),
            Poll::Ready(Ok(Err(err))) => Poll::Ready(Err(err)),
            Poll::Ready(Err(err)) => Poll::Ready(Err(anyhow!(err).context("failed to join task"))),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for Transmission {
    fn drop(&mut self) {
        self.handle.abort()
    }
}

#[derive(Clone, Debug)]
pub struct Client {
    nats: Arc<async_nats::Client>,
    prefix: Arc<String>,
}

impl Client {
    pub fn new(nats: impl Into<Arc<async_nats::Client>>, prefix: impl Into<Arc<String>>) -> Self {
        Self {
            nats: nats.into(),
            prefix: prefix.into(),
        }
    }

    pub fn static_subject(&self, instance: &str, func: &str) -> String {
        let mut s = String::with_capacity(
            self.prefix.len() + PROTOCOL.len() + instance.len() + func.len() + 3,
        );
        if !self.prefix.is_empty() {
            s.push_str(&self.prefix);
            s.push('.');
        }
        s.push_str(PROTOCOL);
        s.push('.');
        if !instance.is_empty() {
            s.push_str(instance);
            s.push('.');
        }
        s.push_str(func);
        s
    }
}

#[derive(Debug)]
pub struct ByteSubscription {
    rx: async_nats::Subscriber,
}

impl Stream for ByteSubscription {
    type Item = anyhow::Result<Bytes>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.rx.poll_next_unpin(cx) {
            Poll::Ready(Some(msg)) => {
                let Message { payload, .. } = msg.try_into()?;
                // TODO: Parse headers
                Poll::Ready(Some(Ok(payload)))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Subject(async_nats::Subject);

impl AsRef<async_nats::Subject> for Subject {
    fn as_ref(&self) -> &async_nats::Subject {
        &self.0
    }
}

impl ToSubject for Subject {
    fn to_subject(&self) -> async_nats::Subject {
        self.0.clone()
    }
}

impl ToSubject for &Subject {
    fn to_subject(&self) -> async_nats::Subject {
        self.0.clone()
    }
}

impl wrpc_transport::Subject for Subject {
    fn child(&self, i: Option<u32>) -> Self {
        if let Some(i) = i {
            Self(format!("{}.{i}", self.0).to_subject())
        } else {
            Self(format!("{}.*", self.0).to_subject())
        }
    }
}

pub struct Subscriber {
    nats: Arc<async_nats::Client>,
}

impl Subscriber {
    pub fn new(nats: Arc<async_nats::Client>) -> Self {
        Self { nats }
    }
}

#[async_trait]
impl wrpc_transport::Subscriber for Subscriber {
    type Subject = Subject;
    type Stream = ByteSubscription;
    type SubscribeError = anyhow::Error;
    type StreamError = anyhow::Error;

    async fn subscribe(
        &self,
        subject: Self::Subject,
    ) -> Result<Self::Stream, Self::SubscribeError> {
        self.nats
            .subscribe(subject.0)
            .await
            .map(|rx| ByteSubscription { rx })
            .context("failed to subscribe")
    }
}

#[derive(Clone, Debug)]
pub struct Transmitter {
    nats: Arc<async_nats::Client>,
}

#[async_trait]
impl wrpc_transport::Transmitter for Transmitter {
    type Subject = Subject;
    type PublishError = async_nats::PublishError;

    #[instrument(level = "trace", ret, skip(self))]
    async fn transmit(
        &self,
        subject: Self::Subject,
        payload: Bytes,
    ) -> Result<(), Self::PublishError> {
        let mut tail = payload;
        let Subject(subject) = subject;
        while !tail.is_empty() {
            let ServerInfo { max_payload, .. } = self.nats.server_info();
            let mut payload = tail;
            tail = if payload.len() > max_payload {
                assert!(max_payload > 0);
                payload.split_off(max_payload)
            } else {
                Bytes::default()
            };
            trace!(?tail, ?payload, max_payload, "publish payload chunk");
            self.nats.publish(subject.clone(), payload).await?;
        }
        Ok(())
    }
}

pub struct Invocation {
    client: Client,
    rx: async_nats::Subject,
}

impl Invocation {
    pub fn new(client: Client, rx: async_nats::Subject) -> Self {
        Self { client, rx }
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn transmit_subject(&self) -> &async_nats::Subject {
        &self.rx
    }

    pub async fn begin<T, U>(self, params: T) -> anyhow::Result<InvocationPre<U>>
    where
        T: IntoIterator<Item = U>,
        T::IntoIter: ExactSizeIterator<Item = U>,
        U: Encode<U>,
    {
        let ((payload, txs), handshake) = try_join!(
            async {
                let params = params.into_iter();
                let mut payload = BytesMut::new();
                let mut txs = Vec::with_capacity(params.len());
                for p in params {
                    let tx = p
                        .encode(&mut payload)
                        .await
                        .context("failed to encode value")?;
                    txs.push(tx)
                }
                Ok((payload.freeze(), txs))
            },
            async {
                self.client
                    .nats
                    .subscribe(self.rx.clone())
                    .await
                    .context("failed to subscribe on handshake subject")
            },
        )?;
        Ok(InvocationPre {
            client: self.client,
            txs,
            payload,
            handshake,
            rx: self.rx,
        })
    }
}

#[async_trait]
impl wrpc_transport::Invocation for Invocation {
    type Transmission = Transmission;
    type TransmissionFailed = Box<dyn Future<Output = ()> + Send + Unpin>;

    async fn invoke<T, U>(
        self,
        instance: &str,
        name: &str,
        params: T,
    ) -> anyhow::Result<(Self::Transmission, Self::TransmissionFailed)>
    where
        T: IntoIterator<Item = U> + Send,
        T::IntoIter: ExactSizeIterator<Item = U> + Send,
        U: Encode<U> + Send + 'static,
    {
        let subject = self.client.static_subject(instance, name);
        let inv = self.begin(params).await?;
        let (tx, tx_failed) = inv.invoke(subject).await?;
        Ok((tx, Box::new(tx_failed)))
    }
}

pub struct InvocationPre<T> {
    client: Client,
    txs: Vec<Option<AsyncTransmission<T>>>,
    rx: async_nats::Subject,
    payload: Bytes,
    handshake: async_nats::Subscriber,
}

impl<T> InvocationPre<T> {
    pub fn payload(&self) -> &Bytes {
        &self.payload
    }

    pub fn payload_mut(&mut self) -> &mut Bytes {
        &mut self.payload
    }
}

impl<T> InvocationPre<T>
where
    T: Encode<T> + Send + 'static,
{
    #[instrument(level = "trace", skip_all)]
    pub async fn invoke(
        mut self,
        subject: impl ToSubject,
    ) -> anyhow::Result<(Transmission, impl Future<Output = ()>)> {
        let ServerInfo { max_payload, .. } = self.client.nats.server_info();
        let payload = if self.payload.len() > max_payload {
            ensure!(max_payload > 0);
            trace!(
                payload = ?self.payload,
                max_payload,
                "payload length exceeds maximum, truncate"
            );
            self.payload.split_to(max_payload)
        } else {
            Bytes::default()
        };
        trace!(rx = ?self.rx, payload = ?self.payload, "publish handshake");
        self.client
            .nats
            .publish_with_reply(subject, self.rx.clone(), payload)
            .await
            .context("failed to publish handshake")?;
        self.finish().await
    }

    #[instrument(level = "trace", skip_all)]
    pub async fn invoke_with_headers(
        mut self,
        subject: impl ToSubject,
        headers: HeaderMap,
    ) -> anyhow::Result<(Transmission, impl Future<Output = ()>)> {
        let ServerInfo { max_payload, .. } = self.client.nats.server_info();
        let payload = if self.payload.len() > max_payload {
            ensure!(max_payload > 0);
            trace!(
                payload = ?self.payload,
                max_payload,
                "payload length exceeds maximum, truncate"
            );
            self.payload.split_to(max_payload)
        } else {
            Bytes::default()
        };
        trace!(rx = ?self.rx, payload = ?self.payload, "publish handshake");
        self.client
            .nats
            .publish_with_reply_and_headers(subject, self.rx.clone(), headers, payload)
            .await
            .context("failed to publish handshake")?;
        self.finish().await
    }

    async fn finish(mut self) -> anyhow::Result<(Transmission, impl Future<Output = ()>)> {
        let (err_tx, err_rx) = oneshot::channel();
        let tx = spawn(async move {
            let res: anyhow::Result<()> = async move {
                trace!("await handshake response");
                let msg = self
                    .handshake
                    .next()
                    .await
                    .context("failed to receive handshake response")?;
                let Message { reply, .. } = msg.try_into()?;
                let reply = reply.map(Subject);
                let tx = Transmitter {
                    nats: self.client.nats,
                };
                let txs: FuturesUnordered<_> = zip(0.., self.txs)
                    .filter_map(|(i, v)| {
                        let v = v?;
                        Some((i, v))
                    })
                    .map(|(i, v)| {
                        let reply = reply
                            .as_ref()
                            .context("peer did not specify a reply inbox")?;
                        let reply = reply.child(Some(i));
                        let tx = tx.clone();
                        let fut: Pin<Box<dyn Future<Output = _> + Send>> = Box::pin(async move {
                            tx.transmit_async(reply, v).await.with_context(|| {
                                format!("failed to transmit asynchronous parameter {i}")
                            })
                        });
                        Ok(fut)
                    })
                    .collect::<anyhow::Result<_>>()?;
                try_join!(
                    async {
                        try_join_all(txs).await?;
                        Ok(())
                    },
                    async {
                        if !self.payload.is_empty() {
                            trace!(payload = ?self.payload, "transmit payload tail");
                            let reply = reply.context("peer did not specify a reply inbox")?;
                            tx.transmit(reply, self.payload)
                                .await
                                .context("failed to send parameter payload to peer")
                        } else {
                            Ok(())
                        }
                    }
                )?;
                Ok(())
            }
            .await;
            match res {
                Ok(()) => Ok(err_tx),
                Err(err) => {
                    _ = err_tx.send(());
                    Err(err)
                }
            }
        });
        Ok((Transmission { handle: tx }, err_rx.map(|_| ())))
    }
}

pub struct Acceptor {
    nats: Arc<async_nats::Client>,
    tx: async_nats::Subject,
}

#[async_trait]
impl wrpc_transport::Acceptor for Acceptor {
    type Subject = Subject;
    type Transmitter = Transmitter;

    async fn accept(self, rx: Self::Subject) -> anyhow::Result<(Self::Subject, Self::Transmitter)> {
        self.nats
            .publish_with_reply(self.tx.clone(), rx, Bytes::default())
            .await
            .context("failed to connect to peer")?;
        Ok((
            Subject(format!("{}.results", self.tx).into()),
            Transmitter { nats: self.nats },
        ))
    }
}

#[async_trait]
impl wrpc_transport::Client for Client {
    type Subject = Subject;
    type Subscriber = Subscriber;
    type Transmission = Transmission;
    type Acceptor = Acceptor;
    type InvocationStream = Pin<
        Box<
            dyn Stream<
                    Item = anyhow::Result<(Bytes, Self::Subject, Self::Subscriber, Self::Acceptor)>,
                > + Send,
        >,
    >;
    type Invocation = Invocation;

    #[instrument(level = "trace", skip(self))]
    async fn serve(&self, instance: &str, func: &str) -> anyhow::Result<Self::InvocationStream> {
        let nats = Arc::clone(&self.nats);
        let invocations = nats
            .subscribe(self.static_subject(instance, func))
            .await
            .context("failed to subscribe on invocation subject")?;
        Ok(Box::pin(invocations.then({
            move |msg| {
                let nats = Arc::clone(&nats);
                async move {
                    let Request { payload, tx, .. } = Request::try_from(msg)?;
                    let rx = nats.new_inbox().to_subject();
                    Ok((
                        payload,
                        Subject(rx.clone()),
                        Subscriber::new(Arc::clone(&nats)),
                        Acceptor { nats, tx },
                    ))
                }
            }
        })))
    }

    fn new_invocation(
        &self,
    ) -> (
        Self::Invocation,
        Self::Subscriber,
        Self::Subject,
        Self::Subject,
    ) {
        let rx = self.nats.new_inbox().to_subject();
        let sub = Subscriber::new(Arc::clone(&self.nats));
        let results = Subject(format!("{rx}.results").into());
        let error = Subject(format!("{rx}.error").into());
        (Invocation::new(self.clone(), rx), sub, results, error)
    }
}
