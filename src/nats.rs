use core::mem;
use core::num::NonZeroUsize;

use core::pin::Pin;
use core::task::{Context, Poll};

use anyhow::{bail, Context as _};
use async_nats::subject::ToSubject;
use async_nats::{Client, HeaderMap, ServerInfo, StatusCode, Subject, SubscribeError, Subscriber};
use bytes::{Bytes, BytesMut};
use futures::{Stream, StreamExt};
use tokio::try_join;
use tracing::{instrument, trace};

#[derive(Debug)]
pub struct Publisher {
    subject: Subject,
    state: StreamState,
}

impl Publisher {
    pub fn sent(&self) -> usize {
        self.state.sent
    }

    pub fn buffer(&self) -> &Bytes {
        &self.state.buffer
    }

    pub fn buffer_mut(&mut self) -> &mut Bytes {
        &mut self.state.buffer
    }

    pub fn sized(self) -> SizedPublisher {
        SizedPublisher(self)
    }

    pub fn unsize(self) -> UnsizedPublisher {
        UnsizedPublisher(self)
    }

    pub fn child_path(&self, path: &str) -> Subject {
        format!("{}.{path}", self.subject).to_subject()
    }

    pub fn child(&self, path: &str) -> Publisher {
        Self {
            subject: self.child_path(path),
            state: StreamState::default(),
        }
    }
}

#[derive(Debug)]
pub struct SizedPublisher(Publisher);

#[derive(Debug)]
pub struct UnsizedPublisher(Publisher);

#[derive(Default, Debug)]
struct StreamState {
    buffer: Bytes,
    sent: usize,
}

impl StreamState {
    fn new(nats: &Client, payload: &mut Bytes) -> Self {
        let ServerInfo { max_payload, .. } = nats.server_info();
        let size = payload.len();
        if size > max_payload {
            let buffer = payload.split_off(max_payload);
            StreamState {
                buffer,
                sent: max_payload,
            }
        } else {
            StreamState {
                buffer: Bytes::new(),
                sent: 0,
            }
        }
    }

    fn publisher(self, subject: Subject) -> Publisher {
        Publisher {
            subject,
            state: self,
        }
    }

    fn unsized_publisher(self, subject: Subject) -> UnsizedPublisher {
        self.publisher(subject).unsize()
    }

    fn sized_publisher(self, subject: Subject) -> Option<SizedPublisher> {
        if self.buffer.is_empty() {
            None
        } else {
            Some(self.publisher(subject).sized())
        }
    }
}

impl SizedPublisher {
    pub async fn flush(&mut self, nats: &Client) -> anyhow::Result<()> {
        let ServerInfo { max_payload, .. } = nats.server_info();
        let max_payload = NonZeroUsize::new(max_payload).context("maximum server payload is 0")?;
        let size = self
            .0
            .state
            .sent
            .checked_add(self.0.state.buffer.len())
            .context("payload size overflow")?;
        loop {
            let len = self.0.state.buffer.len().min(max_payload.into());
            if len == 0 {
                return Ok(());
            }
            let payload = self.0.state.buffer.split_to(len);
            let mut headers = HeaderMap::new();
            let end = self
                .0
                .state
                .sent
                .checked_add(payload.len())
                .context("range length overflow")?;
            headers.insert(
                "Content-Range",
                format!("bytes {}-{end}/{size}", self.0.state.sent).as_str(),
            );
            nats.publish_with_headers(self.0.subject.clone(), headers, payload)
                .await
                .context("failed to publish message")?;
            self.0.state.sent = end;
        }
    }
}

impl UnsizedPublisher {
    pub async fn publish(&mut self, nats: &Client, mut payload: Bytes) -> anyhow::Result<()> {
        let ServerInfo { max_payload, .. } = nats.server_info();
        let buffered = self.0.state.buffer.len();
        let payload = if buffered == 0 {
            if payload.len() > max_payload {
                self.0.state.buffer = payload.split_off(max_payload);
            }
            payload
        } else if max_payload == buffered {
            mem::replace(&mut self.0.state.buffer, payload)
        } else if let Some(cap) = max_payload.checked_sub(buffered) {
            let tail = payload.split_off(cap.min(payload.len()));
            let mut head = BytesMut::with_capacity(cap);
            head.extend_from_slice(&self.0.state.buffer);
            head.extend_from_slice(&payload);
            self.0.state.buffer = tail;
            head.freeze()
        } else {
            let head = self.0.state.buffer.split_to(max_payload);
            let cap = self
                .0
                .state
                .buffer
                .len()
                .checked_add(payload.len())
                .context("buffer length overflow")?;
            let mut tail = BytesMut::with_capacity(cap);
            tail.extend_from_slice(&self.0.state.buffer);
            tail.extend_from_slice(&payload);
            self.0.state.buffer = tail.freeze();
            head
        };
        let mut headers = HeaderMap::new();
        let end = self
            .0
            .state
            .sent
            .checked_add(payload.len())
            .context("range length overflow")?;
        headers.insert(
            "Content-Range",
            format!("bytes {}-{end}/*", self.0.state.sent).as_str(),
        );
        nats.publish_with_headers(self.0.subject.clone(), headers, payload)
            .await
            .context("failed to publish message")?;
        self.0.state.sent = end;
        Ok(())
    }

    pub async fn finish(mut self, nats: &Client, payload: Bytes) -> anyhow::Result<()> {
        let buffered = self.0.state.buffer.len();
        self.0.state.buffer = if buffered == 0 && payload.is_empty() {
            let mut headers = HeaderMap::new();
            headers.insert(
                "Content-Range",
                format!("bytes */{}", self.0.state.sent).as_str(),
            );
            return nats
                .publish_with_headers(self.0.subject, headers, payload)
                .await
                .context("failed to publish message");
        } else if payload.is_empty() {
            self.0.state.buffer
        } else if buffered == 0 {
            payload
        } else {
            let cap = buffered
                .checked_add(payload.len())
                .context("buffer length overflow")?;
            let mut buffer = BytesMut::with_capacity(cap);
            buffer.extend_from_slice(&self.0.state.buffer);
            buffer.extend_from_slice(&payload);
            buffer.freeze()
        };
        SizedPublisher(self.0).flush(nats).await
    }
}

#[derive(Debug)]
pub struct Session {
    state: StreamState,
    tx: Option<Subject>,
    sub_root: Subscriber,
    sub_child: Subscriber,
}

impl Session {
    pub fn peer_inbox(&self) -> Option<&Subject> {
        self.tx.as_ref()
    }

    pub fn split(self) -> (Option<Publisher>, Subscriber, Subscriber) {
        (
            self.tx.map(|tx| self.state.publisher(tx)),
            self.sub_root,
            self.sub_child,
        )
    }

    pub fn split_unsized(self) -> (Option<UnsizedPublisher>, Subscriber, Subscriber) {
        (
            self.tx.map(|tx| self.state.unsized_publisher(tx)),
            self.sub_root,
            self.sub_child,
        )
    }

    pub fn split_sized(self) -> (Option<SizedPublisher>, Subscriber, Subscriber) {
        (
            self.tx.and_then(|tx| self.state.sized_publisher(tx)),
            self.sub_root,
            self.sub_child,
        )
    }
}

#[instrument(level = "trace", skip_all)]
async fn handshake(
    nats: &Client,
    subject: impl ToSubject,
    mut payload: Bytes,
    headers: Option<HeaderMap>,
) -> anyhow::Result<(Subscriber, Subscriber, StreamState)> {
    let reply = nats.new_inbox().to_subject();
    let (sub, sub_child) = try_join!(
        nats.subscribe(reply.clone()),
        nats.subscribe(format!("{reply}.>"))
    )
    .context("failed to subscribe to inbox subjects")?;
    let state = StreamState::new(nats, &mut payload);
    trace!(?reply, "initialize connection");
    if let Some(headers) = headers {
        nats.publish_with_reply_and_headers(subject, reply, headers, payload)
            .await
    } else {
        nats.publish_with_reply(subject, reply, payload).await
    }
    .context("failed to connect to peer")?;
    Ok((sub, sub_child, state))
}

#[instrument(level = "trace", skip_all)]
pub async fn connect(
    nats: &Client,
    subject: impl ToSubject,
    payload: Bytes,
    headers: Option<HeaderMap>,
) -> anyhow::Result<Response> {
    let (mut sub_root, sub_child, state) = handshake(nats, subject, payload, headers).await?;
    let msg = sub_root
        .next()
        .await
        .context("failed to receive peer response")?;
    let Message {
        payload,
        headers,
        subject,
        reply,
    } = msg.try_into()?;
    if reply.is_none() && !state.buffer.is_empty() {
        bail!("payload length exceeded limit, but no reply subject was returned by peer")
    }
    Ok(Response {
        payload,
        headers,
        subject,
        conn: Session {
            state,
            tx: reply,
            sub_root,
            sub_child,
        },
    })
}

#[derive(Debug)]
pub struct Accept {
    reply: Subject,
}

impl Accept {
    pub fn peer_inbox(&self) -> &Subject {
        &self.reply
    }

    #[instrument(level = "trace", skip_all)]
    pub async fn connect(
        self,
        nats: &Client,
        payload: Bytes,
        headers: Option<HeaderMap>,
    ) -> anyhow::Result<Session> {
        let (sub_root, sub_child, state) =
            handshake(nats, self.reply.clone(), payload, headers).await?;
        Ok(Session {
            state,
            tx: Some(self.reply),
            sub_root,
            sub_child,
        })
    }

    pub fn publisher(self) -> Publisher {
        Publisher::from(self)
    }

    #[instrument(level = "trace", skip_all, ret)]
    pub async fn finish(
        self,
        nats: &Client,
        mut payload: Bytes,
        headers: Option<HeaderMap>,
    ) -> anyhow::Result<Option<SizedPublisher>> {
        let state = StreamState::new(nats, &mut payload);
        if let Some(headers) = headers {
            nats.publish_with_headers(self.reply.clone(), headers, payload)
                .await
        } else {
            nats.publish(self.reply.clone(), payload).await
        }
        .context("failed to connect to peer")?;
        Ok(state.sized_publisher(self.reply))
    }
}

impl From<Accept> for Publisher {
    fn from(Accept { reply }: Accept) -> Self {
        Self {
            subject: reply,
            state: StreamState::default(),
        }
    }
}

#[derive(Debug)]
struct Message {
    pub payload: Bytes,
    pub headers: Option<HeaderMap>,
    pub subject: Subject,
    pub reply: Option<Subject>,
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
    pub subject: Subject,
    pub conn: Accept,
}

#[derive(Debug)]
pub struct Response {
    pub payload: Bytes,
    pub headers: Option<HeaderMap>,
    pub subject: Subject,
    pub conn: Session,
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
            conn: Accept { reply },
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

pub async fn accept(sub: &mut Subscriber) -> anyhow::Result<Request> {
    let msg = sub.next().await.context("failed to accept connection")?;
    msg.try_into()
}

#[derive(Debug)]
pub struct Listener {
    sub: Subscriber,
}

impl Listener {
    pub fn new(sub: Subscriber) -> Self {
        Self { sub }
    }
}

impl Stream for Listener {
    type Item = anyhow::Result<Request>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.sub).poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Ready(Some(msg)) => {
                let req = msg.try_into()?;
                Poll::Ready(Some(Ok(req)))
            }
        }
    }
}

pub async fn listen(nats: &Client, subject: impl ToSubject) -> Result<Listener, SubscribeError> {
    let sub = nats.subscribe(subject).await?;
    Ok(Listener { sub })
}
