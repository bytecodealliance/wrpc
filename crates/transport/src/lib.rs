use core::array;
use core::borrow::Borrow;
use core::fmt::Debug;
use core::future::{ready, Future};
use core::iter::zip;
use core::pin::{pin, Pin};
use core::task::{self, Poll};

use std::error::Error;
use std::sync::Arc;

use anyhow::{anyhow, bail, ensure, Context as _};
use async_trait::async_trait;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use futures::future::{poll_immediate, try_join_all};
use futures::stream::FuturesUnordered;
use futures::{stream, Stream, StreamExt as _, TryStreamExt as _};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::{select, spawn, try_join};
use tokio_stream::wrappers::ReceiverStream;
use tracing::{instrument, trace};
use wrpc_types::{Resource, Type};

pub const PROTOCOL: &str = "wrpc.0.0.1";

#[async_trait]
pub trait Transmitter {
    type Subject: Subject + Send + Sync + Clone;
    type PublishError: Error + Send + Sync + 'static;

    async fn transmit(
        &self,
        subject: Self::Subject,
        payload: Bytes,
    ) -> Result<(), Self::PublishError>;

    #[instrument(level = "trace", ret, skip_all)]
    async fn transmit_tuple_dynamic<T>(
        &self,
        subject: Self::Subject,
        values: T,
    ) -> anyhow::Result<()>
    where
        T: IntoIterator<Item = Value> + Send,
        T::IntoIter: ExactSizeIterator<Item = Value> + Send,
    {
        let values = values.into_iter();
        let mut buf = BytesMut::new();
        let mut nested = Vec::with_capacity(values.len());
        for v in values {
            trace!("encode tuple element");
            let tx = v.encode(&mut buf).await.context("failed to encode value")?;
            nested.push(tx)
        }
        let nested: FuturesUnordered<_> = zip(0.., nested)
            .filter_map(|(i, v)| {
                let v = v?;
                let subject = subject.child(Some(i));
                let fut: Pin<Box<dyn Future<Output = _> + Send>> = Box::pin(async move {
                    trace!(i, "transmit asynchronous tuple element value");
                    self.transmit_async(subject, v)
                        .await
                        .with_context(|| format!("failed to transmit asynchronous element {i}"))
                });
                Some(fut)
            })
            .collect();
        try_join!(
            async {
                try_join_all(nested).await?;
                Ok(())
            },
            async {
                trace!("encode tuple value");
                self.transmit(subject, buf.freeze())
                    .await
                    .context("failed to transmit value")
            },
        )?;
        Ok(())
    }

    #[instrument(level = "trace", ret, skip_all)]
    async fn transmit_async<V>(
        &self,
        subject: Self::Subject,
        value: AsyncTransmission<V>,
    ) -> anyhow::Result<()>
    where
        V: Encode<V> + Send,
    {
        match value {
            AsyncTransmission::List(nested)
            | AsyncTransmission::Record(nested)
            | AsyncTransmission::Tuple(nested) => {
                let nested: FuturesUnordered<_> = zip(0.., nested)
                    .filter_map(|(i, v)| {
                        let v = v?;
                        let subject = subject.child(Some(i));
                        let fut: Pin<Box<dyn Future<Output = _> + Send>> = Box::pin(async move {
                            trace!(i, "transmit asynchronous element value");
                            self.transmit_async(subject, v).await.with_context(|| {
                                format!("failed to transmit asynchronous element {i}")
                            })
                        });
                        Some(fut)
                    })
                    .collect();
                try_join_all(nested).await?;
                Ok(())
            }
            AsyncTransmission::Variant {
                discriminant,
                nested,
            } => {
                trace!(discriminant, "transmit asynchronous variant value");
                self.transmit_async(subject.child(Some(discriminant)), *nested)
                    .await
            }
            AsyncTransmission::Option(nested) => {
                trace!("transmit asynchronous option value");
                self.transmit_async(subject.child(Some(1)), *nested)
                    .await
                    .context("failed to transmit asynchronous `option::some` value")
            }
            AsyncTransmission::ResultOk(nested) => {
                trace!("transmit asynchronous result::ok value");
                self.transmit_async(subject.child(Some(0)), *nested)
                    .await
                    .context("failed to transmit asynchronous `result::ok` value")
            }
            AsyncTransmission::ResultErr(nested) => {
                trace!("transmit asynchronous result::err value");
                self.transmit_async(subject.child(Some(1)), *nested)
                    .await
                    .context("failed to transmit asynchronous `result::err` value")
            }
            AsyncTransmission::Future(v) => {
                if let Some(v) = v.await.context("failed to acquire future value")? {
                    let mut payload = BytesMut::new();
                    trace!("encode nested future value");
                    let tx = v
                        .encode(&mut payload)
                        .await
                        .context("failed to encode future value")?;
                    let payload = payload.freeze();
                    let nested = subject.child(Some(0));
                    try_join!(
                        async {
                            if let Some(tx) = tx {
                                trace!("transmit nested asynchronous future value");
                                self.transmit_async(nested, tx)
                                    .await
                                    .context("failed to transmit nested future value")
                            } else {
                                Ok(())
                            }
                        },
                        async {
                            self.transmit(subject, payload)
                                .await
                                .context("failed to transmit future value")
                        },
                    )?;
                    Ok(())
                } else {
                    trace!("transmit empty future value");
                    self.transmit(subject, Bytes::default())
                        .await
                        .context("failed to transmit value to peer")
                }
            }
            AsyncTransmission::Stream(mut v) => {
                // TODO: Batch items
                let mut i = 0;
                loop {
                    let item = v
                        .next()
                        .await
                        .context("stream unexpectedly finished")?
                        .context("failed to receive item")?;
                    match item {
                        StreamItem::End(None) => {
                            self.transmit(subject, Bytes::from_static(&[0])).await?;
                            return Ok(());
                        }
                        StreamItem::End(Some(v)) => {
                            let mut payload = BytesMut::from([0].as_slice());
                            let tx = v
                                .encode(&mut payload)
                                .await
                                .context("failed to encode stream end value")?;
                            let payload = payload.freeze();
                            let nested = subject.child(Some(i)).child(Some(0));
                            try_join!(
                                async {
                                    if let Some(tx) = tx {
                                        trace!("transmit nested asynchronous stream end value");
                                        self.transmit_async(nested, tx)
                                            .await
                                            .context("failed to transmit nested stream end value")
                                    } else {
                                        Ok(())
                                    }
                                },
                                async {
                                    self.transmit(subject, payload)
                                        .await
                                        .context("failed to transmit stream end value")
                                },
                            )?;
                            return Ok(());
                        }
                        StreamItem::Element(None) => {
                            self.transmit(subject.clone(), Bytes::from_static(&[1]))
                                .await?;
                            i += 1;
                        }
                        StreamItem::Element(Some(v)) => {
                            let mut payload = BytesMut::from([1].as_slice());
                            let tx = v
                                .encode(&mut payload)
                                .await
                                .context("failed to encode stream element value")?;
                            let payload = payload.freeze();
                            let nested = subject.child(Some(i)).child(Some(0));
                            try_join!(
                                async {
                                    if let Some(tx) = tx {
                                        trace!("transmit nested asynchronous stream element value");
                                        self.transmit_async(nested, tx).await.context(
                                            "failed to transmit nested stream element value",
                                        )
                                    } else {
                                        Ok(())
                                    }
                                },
                                async {
                                    self.transmit(subject.clone(), payload)
                                        .await
                                        .context("failed to transmit stream element value")
                                },
                            )?;
                            i += 1;
                        }
                    }
                }
            }
        }
    }
}

#[derive(Debug)]
pub enum AsyncSubscription<T> {
    List(Box<AsyncSubscription<T>>),
    Record(Vec<Option<AsyncSubscription<T>>>),
    Tuple(Vec<Option<AsyncSubscription<T>>>),
    Variant(Vec<Option<AsyncSubscription<T>>>),
    Option(Box<AsyncSubscription<T>>),
    Result {
        ok: Option<Box<AsyncSubscription<T>>>,
        err: Option<Box<AsyncSubscription<T>>>,
    },
    Future {
        subscriber: T,
        nested: Option<Box<AsyncSubscription<T>>>,
    },
    Stream {
        subscriber: T,
        nested_element: Option<Box<AsyncSubscription<T>>>,
        nested_end: Option<Box<AsyncSubscription<T>>>,
    },
}

impl<T> AsyncSubscription<T> {
    #[instrument(level = "trace", skip_all)]
    pub fn try_unwrap_list(self) -> anyhow::Result<AsyncSubscriptionDemux<T>> {
        match self {
            AsyncSubscription::List(sub) => sub.demux(),
            _ => bail!("list subscription type mismatch"),
        }
    }

    #[instrument(level = "trace", skip_all)]
    pub fn try_unwrap_record(self) -> anyhow::Result<Vec<Option<AsyncSubscription<T>>>> {
        match self {
            AsyncSubscription::Record(sub) => Ok(sub),
            _ => bail!("record subscription type mismatch"),
        }
    }

    #[instrument(level = "trace", skip_all)]
    pub fn try_unwrap_tuple(self) -> anyhow::Result<Vec<Option<AsyncSubscription<T>>>> {
        match self {
            AsyncSubscription::Tuple(sub) => Ok(sub),
            _ => bail!("tuple subscription type mismatch"),
        }
    }

    #[instrument(level = "trace", skip_all)]
    pub fn try_unwrap_variant(self) -> anyhow::Result<Vec<Option<AsyncSubscription<T>>>> {
        match self {
            AsyncSubscription::Variant(sub) => Ok(sub),
            _ => bail!("variant subscription type mismatch"),
        }
    }

    #[instrument(level = "trace", skip_all)]
    pub fn try_unwrap_option(self) -> anyhow::Result<AsyncSubscription<T>> {
        match self {
            AsyncSubscription::Option(sub) => Ok(*sub),
            _ => bail!("option subscription type mismatch"),
        }
    }

    #[instrument(level = "trace", skip_all)]
    pub fn try_unwrap_result(
        self,
    ) -> anyhow::Result<(Option<AsyncSubscription<T>>, Option<AsyncSubscription<T>>)> {
        match self {
            AsyncSubscription::Result { ok, err } => Ok((ok.map(|sub| *sub), err.map(|sub| *sub))),
            _ => bail!("result subscription type mismatch"),
        }
    }

    #[instrument(level = "trace", skip_all)]
    pub fn try_unwrap_future(self) -> anyhow::Result<(T, Option<AsyncSubscription<T>>)> {
        match self {
            AsyncSubscription::Future { subscriber, nested } => {
                Ok((subscriber, nested.map(|sub| *sub)))
            }
            _ => bail!("future subscription type mismatch"),
        }
    }

    #[instrument(level = "trace", skip_all)]
    pub fn try_unwrap_stream(
        self,
    ) -> anyhow::Result<(
        T,
        Option<AsyncSubscriptionDemux<T>>,
        Option<AsyncSubscription<T>>,
    )> {
        match self {
            AsyncSubscription::Stream {
                subscriber,
                nested_element,
                nested_end,
            } => {
                let nested_element = nested_element.map(|sub| sub.demux()).transpose()?;
                Ok((subscriber, nested_element, nested_end.map(|sub| *sub)))
            }
            _ => bail!("stream subscription type mismatch"),
        }
    }
}

pub struct DemuxStream;

impl Stream for DemuxStream {
    type Item = anyhow::Result<Bytes>;

    #[instrument(level = "trace", skip_all)]
    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
    ) -> task::Poll<Option<Self::Item>> {
        unreachable!()
    }
}

pub enum AsyncSubscriptionDemux<T> {
    List(AsyncSubscription<T>),
    Stream {
        element: Option<AsyncSubscription<T>>,
        end: Option<AsyncSubscription<T>>,
    },
}

impl<T> AsyncSubscriptionDemux<T> {
    #[instrument(level = "trace", skip_all)]
    pub fn select(&mut self, i: u64) -> AsyncSubscription<DemuxStream> {
        unreachable!()
    }
}

impl<T> TryFrom<AsyncSubscription<T>> for AsyncSubscriptionDemux<T> {
    type Error = anyhow::Error;

    #[instrument(level = "trace", skip_all)]
    fn try_from(sub: AsyncSubscription<T>) -> Result<Self, Self::Error> {
        match sub {
            AsyncSubscription::List { .. } => bail!("demultiplexing lists not supported yet"),
            AsyncSubscription::Stream { .. } => bail!("demultiplexing streams not supported yet"),
            _ => bail!("subscription type mismatch, only lists and streams can be demultiplexed"),
        }
    }
}

impl<T> AsyncSubscription<T> {
    fn demux(self) -> anyhow::Result<AsyncSubscriptionDemux<T>> {
        self.try_into()
    }
}

pub trait Subject {
    fn child(&self, i: Option<u32>) -> Self;
}

#[async_trait]
pub trait Subscriber {
    type Subject: Subject + Send + Sync + Clone;
    type Stream: Stream<Item = Result<Bytes, Self::StreamError>> + Send + Sync + Unpin;
    type SubscribeError: Send;
    type StreamError: Send;

    async fn subscribe(&self, subject: Self::Subject)
        -> Result<Self::Stream, Self::SubscribeError>;

    #[instrument(level = "trace", skip_all)]
    async fn subscribe_async(
        &self,
        subject: Self::Subject,
        ty: impl Borrow<Type> + Send,
    ) -> Result<Option<AsyncSubscription<Self::Stream>>, Self::SubscribeError> {
        match ty.borrow() {
            Type::Bool
            | Type::U8
            | Type::U16
            | Type::U32
            | Type::U64
            | Type::S8
            | Type::S16
            | Type::S32
            | Type::S64
            | Type::Float32
            | Type::Float64
            | Type::Char
            | Type::String
            | Type::Enum
            | Type::Flags => Ok(None),
            Type::List(ty) => {
                let subs = self
                    .subscribe_async(subject.child(None), ty.as_ref())
                    .await?;
                Ok(subs.map(Box::new).map(AsyncSubscription::List))
            }
            Type::Record(types) => {
                let subs = self.subscribe_async_iter(&subject, types.as_ref()).await?;
                Ok(subs.map(AsyncSubscription::Record))
            }
            Type::Tuple(types) => {
                let subs = self.subscribe_async_iter(&subject, types.as_ref()).await?;
                Ok(subs.map(AsyncSubscription::Tuple))
            }
            Type::Variant(types) => {
                let subs = self
                    .subscribe_async_iter_optional(&subject, types.iter().map(Option::as_ref))
                    .await?;
                Ok(subs.map(AsyncSubscription::Variant))
            }
            Type::Option(ty) => {
                let sub = self
                    .subscribe_async(subject.child(Some(1)), ty.as_ref())
                    .await?;
                Ok(sub.map(Box::new).map(AsyncSubscription::Option))
            }
            Type::Result { ok, err } => {
                let nested = self
                    .subscribe_async_array_optional(
                        &subject,
                        [
                            ok.as_ref().map(AsRef::as_ref),
                            err.as_ref().map(AsRef::as_ref),
                        ],
                    )
                    .await?;
                Ok(nested.map(|[ok, err]| AsyncSubscription::Result {
                    ok: ok.map(Box::new),
                    err: err.map(Box::new),
                }))
            }
            Type::Future(None) => {
                let subscriber = self.subscribe(subject).await?;
                Ok(Some(AsyncSubscription::Future {
                    subscriber,
                    nested: None,
                }))
            }
            Type::Future(Some(ty)) => {
                let nested = subject.child(Some(0));
                let (subscriber, nested) = try_join!(
                    self.subscribe(subject),
                    self.subscribe_async(nested, ty.as_ref())
                )?;
                Ok(Some(AsyncSubscription::Future {
                    subscriber,
                    nested: nested.map(Box::new),
                }))
            }
            Type::Stream { element, end } => {
                let nested = subject.child(None);
                let (subscriber, nested) = try_join!(
                    self.subscribe(subject),
                    self.subscribe_async_array_optional(
                        &nested,
                        [
                            element.as_ref().map(AsRef::as_ref),
                            end.as_ref().map(AsRef::as_ref)
                        ]
                    )
                )?;
                let (nested_element, nested_end) = nested
                    .map(|[nested_element, nested_end]| {
                        (nested_element.map(Box::new), nested_end.map(Box::new))
                    })
                    .unwrap_or_default();
                Ok(Some(AsyncSubscription::Stream {
                    subscriber,
                    nested_element,
                    nested_end,
                }))
            }
            Type::Resource(Resource::Pollable) => {
                self.subscribe_async(subject, &Type::Future(None)).await
            }
            Type::Resource(Resource::InputStream) => {
                self.subscribe_async(
                    subject,
                    &Type::Stream {
                        element: Some(Arc::new(Type::U8)),
                        end: None,
                    },
                )
                .await
            }
            Type::Resource(Resource::OutputStream | Resource::Dynamic(..)) => Ok(None),
        }
    }

    #[instrument(level = "trace", skip_all)]
    async fn subscribe_async_iter(
        &self,
        subject: impl Borrow<Self::Subject> + Send,
        types: impl IntoIterator<Item = impl Borrow<Type> + Send> + Send,
    ) -> Result<Option<Vec<Option<AsyncSubscription<Self::Stream>>>>, Self::SubscribeError> {
        let subs = try_join_all(
            zip(0.., types)
                .map(|(i, ty)| self.subscribe_async(subject.borrow().child(Some(i)), ty)),
        )
        .await?;
        Ok(subs.iter().any(Option::is_some).then_some(subs))
    }

    #[instrument(level = "trace", skip_all)]
    async fn subscribe_async_iter_optional(
        &self,
        subject: impl Borrow<Self::Subject> + Send,
        types: impl IntoIterator<Item = Option<impl Borrow<Type> + Send>> + Send,
    ) -> Result<Option<Vec<Option<AsyncSubscription<Self::Stream>>>>, Self::SubscribeError> {
        let subs = try_join_all(zip(0.., types).map(|(i, ty)| {
            let subject = subject.borrow().child(Some(i));
            async {
                if let Some(ty) = ty {
                    self.subscribe_async(subject, ty).await
                } else {
                    Ok(None)
                }
            }
        }))
        .await?;
        Ok(subs.iter().any(Option::is_some).then_some(subs))
    }

    #[instrument(level = "trace", skip_all)]
    async fn subscribe_async_array<'a, const N: usize>(
        &self,
        subject: impl Borrow<Self::Subject> + Send,
        types: [impl Borrow<Type> + Send + Sync; N],
    ) -> Result<Option<[Option<AsyncSubscription<Self::Stream>>; N]>, Self::SubscribeError> {
        self.subscribe_async_array_optional(subject, types.map(Some))
            .await
    }

    #[instrument(level = "trace", skip_all)]
    async fn subscribe_async_array_optional<'a, const N: usize>(
        &self,
        subject: impl Borrow<Self::Subject> + Send,
        types: [Option<impl Borrow<Type> + Send + Sync>; N],
    ) -> Result<Option<[Option<AsyncSubscription<Self::Stream>>; N]>, Self::SubscribeError> {
        match types.as_slice() {
            [] | [None] | [None, None] | [None, None, None] | [None, None, None, None] => Ok(None),
            [Some(a)] | [Some(a), None] | [Some(a), None, None] | [Some(a), None, None, None] => {
                let mut sub = self
                    .subscribe_async(subject.borrow().child(Some(0)), a.borrow())
                    .await?;
                if sub.is_some() {
                    Ok(Some(array::from_fn(
                        |i| if i == 0 { sub.take() } else { None },
                    )))
                } else {
                    Ok(None)
                }
            }
            [None, Some(b)] | [None, Some(b), None] | [None, Some(b), None, None] => {
                let mut sub = self
                    .subscribe_async(subject.borrow().child(Some(1)), b.borrow())
                    .await?;
                if sub.is_some() {
                    Ok(Some(array::from_fn(
                        |i| if i == 1 { sub.take() } else { None },
                    )))
                } else {
                    Ok(None)
                }
            }
            [None, None, Some(c)] | [None, None, Some(c), None] => {
                let mut sub = self
                    .subscribe_async(subject.borrow().child(Some(2)), c.borrow())
                    .await?;
                if sub.is_some() {
                    Ok(Some(array::from_fn(
                        |i| if i == 2 { sub.take() } else { None },
                    )))
                } else {
                    Ok(None)
                }
            }
            [None, None, None, Some(d)] => {
                let mut sub = self
                    .subscribe_async(subject.borrow().child(Some(3)), d.borrow())
                    .await?;
                if sub.is_some() {
                    Ok(Some(array::from_fn(
                        |i| if i == 3 { sub.take() } else { None },
                    )))
                } else {
                    Ok(None)
                }
            }
            [Some(a), Some(b)] | [Some(a), Some(b), None] | [Some(a), Some(b), None, None] => {
                let (mut a, mut b) = try_join!(
                    self.subscribe_async(subject.borrow().child(Some(0)), a.borrow()),
                    self.subscribe_async(subject.borrow().child(Some(1)), b.borrow()),
                )?;
                if a.is_some() || b.is_some() {
                    Ok(Some(array::from_fn(|i| match i {
                        0 => a.take(),
                        1 => b.take(),
                        _ => None,
                    })))
                } else {
                    Ok(None)
                }
            }
            [Some(a), Some(b), Some(c)] | [Some(a), Some(b), Some(c), None] => {
                let (mut a, mut b, mut c) = try_join!(
                    self.subscribe_async(subject.borrow().child(Some(0)), a.borrow()),
                    self.subscribe_async(subject.borrow().child(Some(1)), b.borrow()),
                    self.subscribe_async(subject.borrow().child(Some(2)), c.borrow()),
                )?;
                if a.is_some() || b.is_some() || c.is_some() {
                    Ok(Some(array::from_fn(|i| match i {
                        0 => a.take(),
                        1 => b.take(),
                        2 => c.take(),
                        _ => None,
                    })))
                } else {
                    Ok(None)
                }
            }
            [Some(a), Some(b), Some(c), Some(d)] => {
                let (mut a, mut b, mut c, mut d) = try_join!(
                    self.subscribe_async(subject.borrow().child(Some(0)), a.borrow()),
                    self.subscribe_async(subject.borrow().child(Some(1)), b.borrow()),
                    self.subscribe_async(subject.borrow().child(Some(2)), c.borrow()),
                    self.subscribe_async(subject.borrow().child(Some(3)), d.borrow()),
                )?;
                if a.is_some() || b.is_some() || c.is_some() || d.is_some() {
                    Ok(Some(array::from_fn(|i| match i {
                        0 => a.take(),
                        1 => b.take(),
                        2 => c.take(),
                        3 => d.take(),
                        _ => None,
                    })))
                } else {
                    Ok(None)
                }
            }
            _ => match self.subscribe_async_iter_optional(subject, types).await? {
                Some(subs) => match subs.try_into() {
                    Ok(subs) => Ok(Some(subs)),
                    Err(_) => panic!("invalid subscription vector generated"),
                },
                None => Ok(None),
            },
        }
    }

    #[instrument(level = "trace", skip_all)]
    async fn subscribe_tuple<'a, const N: usize>(
        &self,
        subject: Self::Subject,
        types: [&'a Type; N],
        payload: Bytes,
    ) -> Result<TupleReceiver<'a, N, Self::Stream>, Self::SubscribeError> {
        let nested = self.subscribe_async_array(subject.clone(), types);
        let rx = self.subscribe(subject);
        let (rx, nested) = try_join!(rx, nested)?;
        Ok(TupleReceiver {
            rx,
            nested,
            types,
            payload,
        })
    }

    #[instrument(level = "trace", skip_all)]
    async fn subscribe_tuple_dynamic<'a>(
        &self,
        subject: Self::Subject,
        types: &'a [Type],
        payload: Bytes,
    ) -> Result<TupleDynamicReceiver<'a, Self::Stream>, Self::SubscribeError> {
        let nested = self.subscribe_async_iter(subject.clone(), types);
        let rx = self.subscribe(subject);
        let (rx, nested) = try_join!(rx, nested)?;
        Ok(TupleDynamicReceiver {
            rx,
            nested: nested.unwrap_or_default(),
            types,
            payload,
        })
    }
}

pub enum StreamItem<T> {
    Element(Option<T>),
    End(Option<T>),
}

pub enum Value {
    Bool(bool),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    S8(i8),
    S16(i16),
    S32(i32),
    S64(i64),
    Float32(f32),
    Float64(f64),
    Char(char),
    String(String),
    List(Vec<Value>),
    Record(Vec<Value>),
    Tuple(Vec<Value>),
    Variant {
        discriminant: u32,
        nested: Option<Box<Value>>,
    },
    Enum(u32),
    Option(Option<Box<Value>>),
    Result(Result<Option<Box<Value>>, Option<Box<Value>>>),
    Flags(u64),
    Future(Pin<Box<dyn Future<Output = anyhow::Result<Option<Value>>> + Send>>),
    Stream(Pin<Box<dyn Stream<Item = anyhow::Result<StreamItem<Value>>> + Send>>),
}

struct StreamValue<T> {
    items: ReceiverStream<anyhow::Result<StreamItem<T>>>,
    producer: JoinHandle<()>,
}

impl<T> Stream for StreamValue<T> {
    type Item = anyhow::Result<StreamItem<T>>;

    #[instrument(level = "trace", skip_all)]
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Option<Self::Item>> {
        pin!(&mut self.items).poll_next(cx)
    }
}

impl<T> Drop for StreamValue<T> {
    fn drop(&mut self) {
        self.producer.abort()
    }
}

pub enum AsyncTransmission<T> {
    List(Vec<Option<AsyncTransmission<T>>>),
    Record(Vec<Option<AsyncTransmission<T>>>),
    Tuple(Vec<Option<AsyncTransmission<T>>>),
    Variant {
        discriminant: u32,
        nested: Box<AsyncTransmission<T>>,
    },
    Option(Box<AsyncTransmission<T>>),
    ResultOk(Box<AsyncTransmission<T>>),
    ResultErr(Box<AsyncTransmission<T>>),
    Future(Pin<Box<dyn Future<Output = anyhow::Result<Option<T>>> + Send>>),
    Stream(Pin<Box<dyn Stream<Item = anyhow::Result<StreamItem<T>>> + Send>>),
}

fn map_tuple_subscription<T>(
    sub: Option<AsyncSubscription<T>>,
) -> anyhow::Result<Vec<Option<AsyncSubscription<T>>>> {
    let sub = sub.map(AsyncSubscription::try_unwrap_tuple).transpose()?;
    Ok(sub.unwrap_or_default())
}

/// Receive bytes until `payload` contains at least `n` bytes
#[instrument(level = "trace", skip(payload, rx))]
pub async fn receive_at_least(
    payload: impl Buf + Send + 'static,
    rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
    n: usize,
) -> anyhow::Result<Box<dyn Buf + Send>> {
    let mut payload: Box<dyn Buf + Send> = Box::new(payload);
    while payload.remaining() < n {
        trace!(remaining = payload.remaining(), "await next payload chunk");
        let chunk = rx
            .try_next()
            .await
            .context("failed to receive payload chunk")?
            .context("unexpected end of stream")?;
        trace!("payload chunk received");
        payload = Box::new(payload.chain(chunk))
    }
    Ok(payload)
}

#[instrument(level = "trace", skip_all)]
pub async fn receive_leb128_unsigned<'a>(
    payload: impl Buf + Send + 'a,
    rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
) -> anyhow::Result<(u64, Box<dyn Buf + Send + 'a>)> {
    let mut payload: Box<dyn Buf + Send> = Box::new(payload);
    let mut buf = vec![];
    loop {
        if payload.remaining() >= 1 {
            let byte = payload.get_u8();
            buf.push(byte);
            if byte & leb128::CONTINUATION_BIT == 0 {
                trace!(len = buf.len(), "decode unsigned LEB128");
                let v =
                    leb128::read::unsigned(&mut buf.as_slice()).context("failed to read LEB128")?;
                trace!(v, "decoded unsigned LEB128");
                return Ok((v, payload));
            }
        } else {
            trace!("await next payload chunk");
            let chunk = rx
                .try_next()
                .await
                .context("failed to receive payload chunk")?
                .context("unexpected end of stream")?;
            trace!("payload chunk received");
            payload = Box::new(payload.chain(chunk))
        }
    }
}

#[instrument(level = "trace", skip_all)]
pub async fn receive_leb128_signed(
    payload: impl Buf + Send + 'static,
    rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
) -> anyhow::Result<(i64, Box<dyn Buf + Send>)> {
    let mut payload: Box<dyn Buf + Send> = Box::new(payload);
    let mut buf = vec![];
    loop {
        if payload.remaining() >= 1 {
            let byte = payload.get_u8();
            buf.push(byte);
            if byte & leb128::CONTINUATION_BIT == 0 {
                trace!(len = buf.len(), "decode signed LEB128");
                let v =
                    leb128::read::signed(&mut buf.as_slice()).context("failed to read LEB128")?;
                trace!(v, "decoded signed LEB128");
                return Ok((v, payload));
            }
        } else {
            trace!("await next payload chunk");
            let chunk = rx
                .try_next()
                .await
                .context("failed to receive payload chunk")?
                .context("unexpected end of stream")?;
            trace!("payload chunk received");
            payload = Box::new(payload.chain(chunk))
        }
    }
}

#[instrument(level = "trace", skip_all)]
pub async fn receive_list_header(
    payload: impl Buf + Send + 'static,
    rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
) -> anyhow::Result<(u32, Box<dyn Buf + Send>)> {
    trace!("decode list length");
    let (len, payload) = receive_leb128_unsigned(payload, rx)
        .await
        .context("failed to decode list length")?;
    let len = len.try_into().context("list length does not fit in u32")?;
    Ok((len, payload))
}

#[instrument(level = "trace", skip_all)]
pub async fn receive_discriminant(
    payload: impl Buf + Send + 'static,
    rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
) -> anyhow::Result<(u32, Box<dyn Buf + Send>)> {
    let (discriminant, payload) = receive_leb128_unsigned(payload, rx)
        .await
        .context("failed to decode discriminant")?;
    let discriminant = discriminant
        .try_into()
        .context("discriminant does not fit in u32")?;
    Ok((discriminant, payload))
}

#[async_trait]
pub trait Receive: Sized {
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static;

    async fn receive_sync(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)> {
        Self::receive(
            payload,
            rx,
            None::<
                AsyncSubscription<
                    Box<dyn Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin>,
                >,
            >,
        )
        .await
    }
}

#[async_trait]
pub trait ReceiveContext<Ctx>: Sized
where
    Ctx: Send + Sync + 'static,
{
    async fn receive_context<T>(
        cx: &Ctx,
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static;

    async fn receive_context_sync(
        cx: &Ctx,
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)> {
        Self::receive_context(
            cx,
            payload,
            rx,
            None::<
                AsyncSubscription<
                    Box<dyn Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin>,
                >,
            >,
        )
        .await
    }

    /// Receive a list
    #[instrument(level = "trace", skip_all)]
    async fn receive_list<T>(
        cx: &Ctx,
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Vec<Self>, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        let mut sub = sub.map(AsyncSubscription::try_unwrap_list).transpose()?;
        let (len, mut payload) = receive_list_header(payload, rx).await?;
        trace!(len, "decode list");
        let cap = len
            .try_into()
            .context("list length does not fit in usize")?;
        let mut els = Vec::with_capacity(cap);
        for i in 0..len {
            trace!(i, "decode list element");
            let sub = sub.as_mut().map(|sub| sub.select(i.into()));
            let el;
            (el, payload) = Self::receive_context(cx, payload, rx, sub)
                .await
                .with_context(|| format!("failed to decode value of list element {i}"))?;
            els.push(el);
        }
        Ok((els, payload))
    }

    async fn receive_stream_item<T>(
        element_cx: Option<&Ctx>,
        end_cx: Option<&Ctx>,
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        nested_element: Option<AsyncSubscription<DemuxStream>>,
        nested_end: &mut Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(StreamItem<Self>, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        let mut payload = receive_at_least(payload, rx, 1).await?;
        match (payload.get_u8(), element_cx, end_cx) {
            (0, _, None) => Ok((StreamItem::End(None), payload)),
            (0, _, Some(cx)) => {
                let (v, payload) =
                    Self::receive_context(cx, payload, rx, nested_end.take()).await?;
                Ok((StreamItem::End(Some(v)), payload))
            }
            (1, None, _) => Ok((StreamItem::Element(None), payload)),
            (1, Some(cx), _) => {
                let (v, payload) = Self::receive_context(cx, payload, rx, nested_element).await?;
                Ok((StreamItem::Element(Some(v)), payload))
            }
            _ => bail!("invalid `stream` variant"),
        }
    }
}

#[async_trait]
impl<R, Ctx> ReceiveContext<Ctx> for R
where
    R: Receive,
    Ctx: Send + Sync + 'static,
{
    async fn receive_context<T>(
        _cx: &Ctx,
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        R::receive(payload, rx, sub).await
    }
}

#[async_trait]
impl Receive for bool {
    #[instrument(level = "trace", skip_all)]
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        let mut payload = receive_at_least(payload, rx, 1).await?;
        trace!("decode bool");
        Ok((payload.get_u8() == 1, payload))
    }
}

#[async_trait]
impl Receive for u8 {
    #[instrument(level = "trace", skip_all)]
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        let mut payload = receive_at_least(payload, rx, 1).await?;
        trace!("decode u8");
        Ok((payload.get_u8(), payload))
    }
}

#[async_trait]
impl Receive for u16 {
    #[instrument(level = "trace", skip_all)]
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        trace!("decode u16");
        let (v, payload) = receive_leb128_unsigned(payload, rx)
            .await
            .context("failed to decode u16")?;
        let v = v
            .try_into()
            .context("received integer value overflows u16")?;
        Ok((v, payload))
    }
}

#[async_trait]
impl Receive for u32 {
    #[instrument(level = "trace", skip_all)]
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        trace!("decode u32");
        let (v, payload) = receive_leb128_unsigned(payload, rx)
            .await
            .context("failed to decode u32")?;
        let v = v
            .try_into()
            .context("received integer value overflows u32")?;
        Ok((v, payload))
    }
}

#[async_trait]
impl Receive for u64 {
    #[instrument(level = "trace", skip_all)]
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        trace!("decode u64");
        let (v, payload) = receive_leb128_unsigned(payload, rx)
            .await
            .context("failed to decode u64")?;
        Ok((v, payload))
    }
}

#[async_trait]
impl Receive for i8 {
    #[instrument(level = "trace", skip_all)]
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        let mut payload = receive_at_least(payload, rx, 1).await?;
        trace!("decode s8");
        Ok((payload.get_i8(), payload))
    }
}

#[async_trait]
impl Receive for i16 {
    #[instrument(level = "trace", skip_all)]
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        trace!("decode s16");
        let (v, payload) = receive_leb128_signed(payload, rx)
            .await
            .context("failed to decode s16")?;
        let v = v
            .try_into()
            .context("received integer value overflows s16")?;
        Ok((v, payload))
    }
}

#[async_trait]
impl Receive for i32 {
    #[instrument(level = "trace", skip_all)]
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        trace!("decode s32");
        let (v, payload) = receive_leb128_signed(payload, rx)
            .await
            .context("failed to decode s32")?;
        let v = v
            .try_into()
            .context("received integer value overflows s32")?;
        Ok((v, payload))
    }
}

#[async_trait]
impl Receive for i64 {
    #[instrument(level = "trace", skip_all)]
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        trace!("decode s64");
        let (v, payload) = receive_leb128_signed(payload, rx)
            .await
            .context("failed to decode s64")?;
        Ok((v, payload))
    }
}

#[async_trait]
impl Receive for f32 {
    #[instrument(level = "trace", skip_all)]
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        trace!("decode float32");
        let mut payload = receive_at_least(payload, rx, 8).await?;
        Ok((payload.get_f32_le(), payload))
    }
}

#[async_trait]
impl Receive for f64 {
    #[instrument(level = "trace", skip_all)]
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        trace!("decode float64");
        let mut payload = receive_at_least(payload, rx, 8).await?;
        Ok((payload.get_f64_le(), payload))
    }
}

#[async_trait]
impl Receive for char {
    #[instrument(level = "trace", skip_all)]
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        trace!("decode char");
        let (v, payload) = receive_leb128_unsigned(payload, rx)
            .await
            .context("failed to decode char")?;
        let v = v
            .try_into()
            .context("received integer value overflows u32")?;
        let v = char::from_u32(v).context("invalid char received")?;
        Ok((v, payload))
    }
}

#[async_trait]
impl Receive for String {
    #[instrument(level = "trace", skip_all)]
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        trace!("decode string length");
        let (len, payload) = receive_leb128_unsigned(payload, rx)
            .await
            .context("failed to decode string length")?;
        let len = len
            .try_into()
            .context("string length does not fit in usize")?;
        let mut payload = receive_at_least(payload, rx, len).await?;
        trace!(len, "decode string");
        let mut buf = vec![0; len];
        payload.copy_to_slice(&mut buf);
        let v = String::from_utf8(buf).context("string is not valid UTF-8")?;
        Ok((v, payload))
    }
}

#[async_trait]
impl<E> Receive for Vec<E>
where
    E: Receive + Send,
{
    #[instrument(level = "trace", skip_all)]
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        let mut sub = sub.map(AsyncSubscription::try_unwrap_list).transpose()?;
        let (len, mut payload) = receive_list_header(payload, rx).await?;
        trace!(len, "decode list");
        let cap = len
            .try_into()
            .context("list length does not fit in usize")?;
        let mut els = Vec::with_capacity(cap);
        for i in 0..len {
            trace!(i, "decode list element");
            let sub = sub.as_mut().map(|sub| sub.select(i.into()));
            let el;
            (el, payload) = E::receive(payload, rx, sub)
                .await
                .with_context(|| format!("failed to decode value of list element {i}"))?;
            els.push(el);
        }
        Ok((els, payload))
    }
}

#[async_trait]
impl Receive for Bytes {
    #[instrument(level = "trace", skip_all)]
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        let (len, payload) = receive_list_header(payload, rx).await?;
        let cap = len
            .try_into()
            .context("list length does not fit in usize")?;
        let mut payload = receive_at_least(payload, rx, cap).await?;
        Ok((payload.copy_to_bytes(cap), payload))
    }
}

#[async_trait]
impl<E> Receive for Option<E>
where
    E: Receive,
{
    #[instrument(level = "trace", skip_all)]
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        let mut payload = receive_at_least(payload, rx, 1).await?;
        trace!("decode option variant");
        match payload.get_u8() {
            0 => Ok((None, payload)),
            1 => {
                let sub = sub.map(AsyncSubscription::try_unwrap_option).transpose()?;
                trace!("decode option value");
                let (v, payload) = E::receive(payload, rx, sub)
                    .await
                    .context("failed to decode option value")?;
                Ok((Some(v), payload))
            }
            _ => bail!("invalid option variant"),
        }
    }
}

#[async_trait]
impl<Ok, Err> Receive for Result<Ok, Err>
where
    Ok: Receive,
    Err: Receive,
{
    #[instrument(level = "trace", skip_all)]
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        let mut payload = receive_at_least(payload, rx, 1).await?;
        let sub = sub.map(AsyncSubscription::try_unwrap_result).transpose()?;
        trace!("decode result variant");
        match payload.get_u8() {
            0 => {
                trace!("decode `result::ok` value");
                let (v, payload) = Ok::receive(payload, rx, sub.and_then(|(ok, _)| ok))
                    .await
                    .context("failed to decode `result::ok` value")?;
                Ok((Ok(v), payload))
            }
            1 => {
                trace!("decode `result::err` value");
                let (v, payload) = Err::receive(payload, rx, sub.and_then(|(_, err)| err))
                    .await
                    .context("failed to decode `result::err` value")?;
                Ok((Err(v), payload))
            }
            _ => bail!("invalid `result` variant"),
        }
    }
}

#[async_trait]
impl<E> Receive for Box<dyn Future<Output = anyhow::Result<E>>>
where
    E: Receive + 'static,
{
    #[instrument(level = "trace", skip_all)]
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        let Some((mut subscriber, nested)) =
            sub.map(AsyncSubscription::try_unwrap_future).transpose()?
        else {
            bail!("future subscription type mismatch")
        };
        let mut payload = receive_at_least(payload, rx, 1).await?;
        match payload.get_u8() {
            0 => Ok((
                Box::new(async move {
                    let (v, _) = E::receive(Bytes::default(), &mut subscriber, nested).await?;
                    Ok(v)
                }),
                payload,
            )),
            1 => {
                let (v, payload) = E::receive(payload, rx, nested).await?;
                Ok((Box::new(ready(Ok(v))), payload))
            }
            _ => bail!("invalid `future` variant"),
        }
    }
}

#[async_trait]
impl Receive for () {
    #[instrument(level = "trace", skip_all)]
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        _rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        Ok(((), Box::new(payload)))
    }
}

#[async_trait]
impl<A> Receive for (A,)
where
    A: Receive,
{
    #[instrument(level = "trace", skip_all)]
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        trace!(i = 0, "decode tuple element");
        let mut sub = map_tuple_subscription(sub)?;
        let (a, payload) = A::receive(payload, rx, sub.get_mut(0).and_then(Option::take)).await?;
        Ok(((a,), payload))
    }
}

#[async_trait]
impl<A, B> Receive for (A, B)
where
    A: Receive + Send,
    B: Receive + Send,
{
    #[instrument(level = "trace", skip_all)]
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        trace!(i = 0, "decode tuple element");
        let mut sub = sub.map(AsyncSubscription::try_unwrap_tuple).transpose()?;
        let (a, payload) = A::receive(
            payload,
            rx,
            sub.as_mut()
                .and_then(|sub| sub.get_mut(0))
                .and_then(Option::take),
        )
        .await?;
        trace!(i = 1, "decode tuple element");
        let (b, payload) = B::receive(
            payload,
            rx,
            sub.as_mut()
                .and_then(|sub| sub.get_mut(1))
                .and_then(Option::take),
        )
        .await?;
        Ok(((a, b), payload))
    }
}

#[async_trait]
impl<A, B, C> Receive for (A, B, C)
where
    A: Receive + Send,
    B: Receive + Send,
    C: Receive + Send,
{
    #[instrument(level = "trace", skip_all)]
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        trace!(i = 0, "decode tuple element");
        let mut sub = sub.map(AsyncSubscription::try_unwrap_tuple).transpose()?;
        let (a, payload) = A::receive(
            payload,
            rx,
            sub.as_mut()
                .and_then(|sub| sub.get_mut(0))
                .and_then(Option::take),
        )
        .await?;
        trace!(i = 1, "decode tuple element");
        let (b, payload) = B::receive(
            payload,
            rx,
            sub.as_mut()
                .and_then(|sub| sub.get_mut(1))
                .and_then(Option::take),
        )
        .await?;
        trace!(i = 2, "decode tuple element");
        let (c, payload) = C::receive(
            payload,
            rx,
            sub.as_mut()
                .and_then(|sub| sub.get_mut(2))
                .and_then(Option::take),
        )
        .await?;
        Ok(((a, b, c), payload))
    }
}

#[async_trait]
impl<A, B, C, D> Receive for (A, B, C, D)
where
    A: Receive + Send,
    B: Receive + Send,
    C: Receive + Send,
    D: Receive + Send,
{
    #[instrument(level = "trace", skip_all)]
    async fn receive<T>(
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        trace!(i = 0, "decode tuple element");
        let mut sub = sub.map(AsyncSubscription::try_unwrap_tuple).transpose()?;
        let (a, payload) = A::receive(
            payload,
            rx,
            sub.as_mut()
                .and_then(|sub| sub.get_mut(0))
                .and_then(Option::take),
        )
        .await?;
        trace!(i = 1, "decode tuple element");
        let (b, payload) = B::receive(
            payload,
            rx,
            sub.as_mut()
                .and_then(|sub| sub.get_mut(1))
                .and_then(Option::take),
        )
        .await?;
        trace!(i = 2, "decode tuple element");
        let (c, payload) = C::receive(
            payload,
            rx,
            sub.as_mut()
                .and_then(|sub| sub.get_mut(2))
                .and_then(Option::take),
        )
        .await?;
        trace!(i = 3, "decode tuple element");
        let (d, payload) = D::receive(
            payload,
            rx,
            sub.as_mut()
                .and_then(|sub| sub.get_mut(3))
                .and_then(Option::take),
        )
        .await?;
        Ok(((a, b, c, d), payload))
    }
}

#[async_trait]
impl ReceiveContext<Type> for Value {
    #[instrument(level = "trace", skip_all)]
    async fn receive_context<T>(
        ty: &Type,
        payload: impl Buf + Send + 'static,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
    {
        match ty {
            Type::Bool => {
                let (v, payload) = bool::receive(payload, rx, sub).await?;
                Ok((Self::Bool(v), payload))
            }
            Type::U8 => {
                let (v, payload) = u8::receive(payload, rx, sub).await?;
                Ok((Self::U8(v), payload))
            }
            Type::U16 => {
                let (v, payload) = u16::receive(payload, rx, sub).await?;
                Ok((Self::U16(v), payload))
            }
            Type::U32 => {
                let (v, payload) = u32::receive(payload, rx, sub).await?;
                Ok((Self::U32(v), payload))
            }
            Type::U64 => {
                let (v, payload) = u64::receive(payload, rx, sub).await?;
                Ok((Self::U64(v), payload))
            }
            Type::S8 => {
                let (v, payload) = i8::receive(payload, rx, sub).await?;
                Ok((Self::S8(v), payload))
            }
            Type::S16 => {
                let (v, payload) = i16::receive(payload, rx, sub).await?;
                Ok((Self::S16(v), payload))
            }
            Type::S32 => {
                let (v, payload) = i32::receive(payload, rx, sub).await?;
                Ok((Self::S32(v), payload))
            }
            Type::S64 => {
                let (v, payload) = i64::receive(payload, rx, sub).await?;
                Ok((Self::S64(v), payload))
            }
            Type::Float32 => {
                let (v, payload) = f32::receive(payload, rx, sub).await?;
                Ok((Self::Float32(v), payload))
            }
            Type::Float64 => {
                let (v, payload) = f64::receive(payload, rx, sub).await?;
                Ok((Self::Float64(v), payload))
            }
            Type::Char => {
                let (v, payload) = char::receive(payload, rx, sub).await?;
                Ok((Self::Char(v), payload))
            }
            Type::String => {
                let (v, payload) = String::receive(payload, rx, sub).await?;
                Ok((Self::String(v.into()), payload))
            }
            Type::List(ty) => {
                let (els, payload) = Self::receive_list(ty, payload, rx, sub).await?;
                Ok((Self::List(els), payload))
            }
            Type::Record(tys) => {
                let mut fields = Vec::with_capacity(tys.len());
                let mut sub = sub.map(AsyncSubscription::try_unwrap_record).transpose()?;
                let mut payload: Box<dyn Buf + Send> = Box::new(payload);
                for (i, ty) in zip(0.., tys.iter()) {
                    trace!(i, "decode record field");
                    let sub = sub
                        .as_mut()
                        .and_then(|sub| sub.get_mut(i))
                        .and_then(Option::take);
                    let el;
                    (el, payload) = Self::receive_context(ty, payload, rx, sub)
                        .await
                        .with_context(|| format!("failed to decode value of record field {i}"))?;
                    fields.push(el);
                }
                Ok((Self::Record(fields), payload))
            }
            Type::Tuple(tys) => {
                let mut els = Vec::with_capacity(tys.len());
                let mut sub = sub.map(AsyncSubscription::try_unwrap_tuple).transpose()?;
                let mut payload: Box<dyn Buf + Send> = Box::new(payload);
                for (i, ty) in zip(0.., tys.iter()) {
                    trace!(i, "decode tuple element");
                    let sub = sub
                        .as_mut()
                        .and_then(|sub| sub.get_mut(i))
                        .and_then(Option::take);
                    let el;
                    (el, payload) = Self::receive_context(ty, payload, rx, sub)
                        .await
                        .with_context(|| format!("failed to decode value of tuple element {i}"))?;
                    els.push(el);
                }
                return Ok((Self::Tuple(els), payload));
            }
            Type::Variant(cases) => {
                trace!("decode variant discriminant");
                let (discriminant, payload) = receive_discriminant(payload, rx)
                    .await
                    .context("failed to decode variant discriminant")?;
                let i: usize = discriminant
                    .try_into()
                    .context("variant discriminant does not fit in usize")?;
                let case = cases.get(i).context("variant discriminant is unknown")?;
                let sub = sub
                    .map(|sub| {
                        let mut sub = sub.try_unwrap_variant()?;
                        anyhow::Ok(sub.get_mut(i).and_then(Option::take))
                    })
                    .transpose()?
                    .flatten();
                if let Some(ty) = case {
                    trace!(discriminant, "decode variant value");
                    let (v, payload) = Self::receive_context(ty, payload, rx, sub)
                        .await
                        .context("failed to decode variant value")?;
                    Ok((
                        Self::Variant {
                            discriminant,
                            nested: Some(Box::new(v)),
                        },
                        payload,
                    ))
                } else {
                    Ok((
                        Self::Variant {
                            discriminant,
                            nested: None,
                        },
                        payload,
                    ))
                }
            }
            Type::Enum => {
                trace!("decode enum discriminant");
                let (discriminant, payload) = receive_discriminant(payload, rx)
                    .await
                    .context("failed to decode enum discriminant")?;
                Ok((Self::Enum(discriminant), payload))
            }
            Type::Option(ty) => {
                let mut payload = receive_at_least(payload, rx, 1).await?;
                trace!("decode option variant");
                match payload.get_u8() {
                    0 => Ok((Self::Option(None), payload)),
                    1 => {
                        let sub = sub.map(AsyncSubscription::try_unwrap_option).transpose()?;
                        trace!("decode option value");
                        let (v, payload) = Self::receive_context(ty, payload, rx, sub)
                            .await
                            .context("failed to decode `option::some` value")?;
                        Ok((Self::Option(Some(Box::new(v))), payload))
                    }
                    _ => bail!("invalid `option` variant"),
                }
            }
            Type::Result { ok, err } => {
                let sub = sub.map(AsyncSubscription::try_unwrap_result).transpose()?;
                let mut payload = receive_at_least(payload, rx, 1).await?;
                trace!("decode result variant");
                match (payload.get_u8(), ok, err) {
                    (0, None, _) => Ok((Self::Result(Ok(None)), payload)),
                    (0, Some(ty), _) => {
                        trace!("decode `result::ok` value");
                        let (v, payload) =
                            Self::receive_context(ty, payload, rx, sub.and_then(|(ok, _)| ok))
                                .await
                                .context("failed to decode `result::ok` value")?;
                        Ok((Self::Result(Ok(Some(Box::new(v)))), payload))
                    }
                    (1, _, None) => Ok((Self::Result(Err(None)), payload)),
                    (1, _, Some(ty)) => {
                        trace!("decode `result::err` value");
                        let (v, payload) =
                            Self::receive_context(ty, payload, rx, sub.and_then(|(_, err)| err))
                                .await
                                .context("failed to decode `result::err` value")?;
                        Ok((Self::Result(Err(Some(Box::new(v)))), payload))
                    }
                    _ => bail!("invalid `result` variant"),
                }
            }
            Type::Flags => {
                trace!("decode flags");
                let (v, payload) = receive_leb128_unsigned(payload, rx)
                    .await
                    .context("failed to decode flags")?;
                Ok((Self::Flags(v), payload))
            }
            Type::Future(ty) => {
                let Some((mut subscriber, nested)) =
                    sub.map(AsyncSubscription::try_unwrap_future).transpose()?
                else {
                    bail!("future subscription type mismatch")
                };
                trace!("decode future");
                let mut payload = receive_at_least(payload, rx, 1).await?;
                trace!("decode future variant");
                match (payload.get_u8(), ty.as_ref()) {
                    (0, None) => {
                        trace!("decoded pending unit future");
                        Ok((
                            Self::Future(Box::pin(async move {
                                trace!("decode future unit value");
                                let buf = subscriber
                                    .try_next()
                                    .await
                                    .context("failed to receive future value")?
                                    .context("future stream unexpectedly closed")?;
                                ensure!(buf.is_empty());
                                Ok(None)
                            })),
                            payload,
                        ))
                    }
                    (0, Some(ty)) => {
                        let ty = Arc::clone(ty);
                        trace!("decoded pending future");
                        Ok((
                            Self::Future(Box::pin(async move {
                                trace!("decode future nested value");
                                let (v, _) = Self::receive_context(
                                    &ty,
                                    Bytes::default(),
                                    &mut subscriber,
                                    nested,
                                )
                                .await?;
                                Ok(Some(v))
                            })),
                            payload,
                        ))
                    }
                    (1, None) => {
                        trace!("decoded ready unit future");
                        Ok((Self::Future(Box::pin(ready(Ok(None)))), payload))
                    }
                    (1, Some(ty)) => {
                        trace!("decode ready future nested value");
                        let (v, payload) = Self::receive_context(ty, payload, rx, nested).await?;
                        Ok((Self::Future(Box::pin(ready(Ok(Some(v))))), payload))
                    }
                    _ => bail!("invalid `future` variant"),
                }
            }
            Type::Stream { element, end } => {
                let Some((mut subscriber, mut nested_element, mut nested_end)) =
                    sub.map(AsyncSubscription::try_unwrap_stream).transpose()?
                else {
                    bail!("stream subscription type mismatch")
                };
                trace!("decode stream");
                let mut payload = receive_at_least(payload, rx, 1).await?;
                trace!(i = 0, "decode stream item variant");
                let byte = payload.copy_to_bytes(1);
                match byte.first().unwrap() {
                    0 => {
                        let (items_tx, items_rx) = mpsc::channel(1);
                        let element = element.as_ref().map(Arc::clone);
                        let end = end.as_ref().map(Arc::clone);
                        let producer = spawn(async move {
                            let mut payload: Box<dyn Buf + Send> = Box::new(Bytes::new());
                            for i in 0.. {
                                match Self::receive_stream_item(
                                    element.as_deref(),
                                    end.as_deref(),
                                    payload,
                                    &mut subscriber,
                                    nested_element.as_mut().map(|sub| sub.select(i)),
                                    &mut nested_end,
                                )
                                .await
                                {
                                    Ok((StreamItem::Element(element), buf)) => {
                                        payload = buf;

                                        if let Err(err) =
                                            items_tx.send(Ok(StreamItem::Element(element))).await
                                        {
                                            trace!(?err, "item receiver closed");
                                            return;
                                        }
                                    }
                                    Ok((StreamItem::End(end), _)) => {
                                        if let Err(err) =
                                            items_tx.send(Ok(StreamItem::End(end))).await
                                        {
                                            trace!(?err, "item receiver closed");
                                            return;
                                        }
                                        return;
                                    }
                                    Err(err) => {
                                        trace!(?err, "stream producer encountered error");
                                        if let Err(err) = items_tx.send(Err(err)).await {
                                            trace!(?err, "item receiver closed");
                                        }
                                        return;
                                    }
                                }
                            }
                        });
                        Ok((
                            Self::Stream(Box::pin(StreamValue {
                                producer,
                                items: ReceiverStream::new(items_rx),
                            })),
                            payload,
                        ))
                    }
                    1 => {
                        let (element, payload) = if let Some(element) = element {
                            trace!(i = 0, "decode stream element");
                            let sub = nested_element.as_mut().map(|sub| sub.select(0));
                            let (v, payload) = Self::receive_context(element, payload, rx, sub)
                                .await
                                .context("failed to decode value of stream element 0")?;
                            (Some(v), payload)
                        } else {
                            (None, payload)
                        };
                        let mut payload = receive_at_least(payload, rx, 1).await?;
                        trace!(i = 1, "decode stream item variant");
                        ensure!(payload.get_u8() == 0);
                        let (end, payload) = if let Some(end) = end {
                            trace!("decode stream end");
                            let (v, payload) =
                                Self::receive_context(end, payload, rx, nested_end).await?;
                            (Some(v), payload)
                        } else {
                            (None, payload)
                        };
                        Ok((
                            Value::Stream(Box::pin(stream::iter([
                                Ok(StreamItem::Element(element)),
                                Ok(StreamItem::End(end)),
                            ]))),
                            payload,
                        ))
                    }
                    _ => {
                        trace!("decode stream length");
                        let (len, mut payload) = receive_leb128_unsigned(byte.chain(payload), rx)
                            .await
                            .context("failed to decode stream length")?;
                        trace!(len, "decode stream elements");
                        let els = if let Some(element) = element {
                            let cap = len
                                .try_into()
                                .context("stream element length does not fit in usize")?;
                            let mut els = Vec::with_capacity(cap);
                            for i in 0..len {
                                trace!(i, "decode stream element");
                                let sub = nested_element.as_mut().map(|sub| sub.select(i));
                                let el;
                                (el, payload) = Self::receive_context(element, payload, rx, sub)
                                    .await
                                    .with_context(|| {
                                        format!("failed to decode value of list element {i}")
                                    })?;
                                els.push(Ok(StreamItem::Element(Some(el))));
                            }
                            els
                        } else {
                            Vec::default()
                        };
                        ensure!(payload.get_u8() == 0);
                        let (end, payload) = if let Some(end) = end {
                            let (v, payload) =
                                Self::receive_context(end, payload, rx, nested_end).await?;
                            (Some(v), payload)
                        } else {
                            (None, payload)
                        };
                        Ok((
                            Value::Stream(Box::pin(stream::iter(
                                els.into_iter().chain([Ok(StreamItem::End(end))]),
                            ))),
                            payload,
                        ))
                    }
                }
            }
            Type::Resource(Resource::Pollable) => {
                Self::receive_context(&Type::Future(None), payload, rx, sub).await
            }
            Type::Resource(Resource::InputStream) => {
                Self::receive_context(
                    &Type::Stream {
                        element: Some(Arc::new(Type::U8)),
                        end: None,
                    },
                    payload,
                    rx,
                    sub,
                )
                .await
            }
            Type::Resource(Resource::OutputStream | Resource::Dynamic(..)) => {
                Self::receive_context(&Type::String, payload, rx, sub)
                    .await
                    .context("failed to decode resource identifer")
            }
        }
    }
}

#[async_trait]
pub trait Encode<T> {
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncTransmission<T>>>;
}

#[async_trait]
impl Encode<Value> for Value {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncTransmission<Value>>> {
        match self {
            Self::Bool(false) => {
                trace!(v = false, "encode bool");
                payload.put_u8(0);
                Ok(None)
            }
            Self::Bool(true) => {
                trace!(v = true, "encode bool");
                payload.put_u8(1);
                Ok(None)
            }
            Self::U8(v) => {
                trace!(v, "encode u8");
                payload.put_u8(v);
                Ok(None)
            }
            Self::U16(v) => {
                trace!(v, "encode u16");
                leb128::write::unsigned(&mut payload.writer(), v.into())
                    .context("failed to encode u16")?;
                Ok(None)
            }
            Self::U32(v) => {
                trace!(v, "encode u32");
                leb128::write::unsigned(&mut payload.writer(), v.into())
                    .context("failed to encode u32")?;
                Ok(None)
            }
            Self::U64(v) => {
                trace!(v, "encode u64");
                leb128::write::unsigned(&mut payload.writer(), v)
                    .context("failed to encode u64")?;
                Ok(None)
            }
            Self::S8(v) => {
                trace!(v, "encode s8");
                payload.put_i8(v);
                Ok(None)
            }
            Self::S16(v) => {
                trace!(v, "encode s16");
                leb128::write::signed(&mut payload.writer(), v.into())
                    .context("failed to encode s16")?;
                Ok(None)
            }
            Self::S32(v) => {
                trace!(v, "encode s32");
                leb128::write::signed(&mut payload.writer(), v.into())
                    .context("failed to encode s32")?;
                Ok(None)
            }
            Self::S64(v) => {
                trace!(v, "encode s64");
                leb128::write::signed(&mut payload.writer(), v).context("failed to encode s64")?;
                Ok(None)
            }
            Self::Float32(v) => {
                trace!(v, "encode float32");
                payload.put_f32_le(v);
                Ok(None)
            }
            Self::Float64(v) => {
                trace!(v, "encode float64");
                payload.put_f64_le(v);
                Ok(None)
            }
            Self::Char(v) => {
                trace!(?v, "encode char");
                leb128::write::unsigned(&mut payload.writer(), v.into())
                    .context("failed to encode char")?;
                Ok(None)
            }
            Self::String(v) => {
                trace!(len = v.len(), "encode string length");
                let len = v
                    .len()
                    .try_into()
                    .context("string length does not fit in u64")?;
                leb128::write::unsigned(&mut payload.writer(), len)
                    .context("failed to encode string length")?;
                trace!(v, "encode string value");
                payload.put_slice(v.as_bytes());
                Ok(None)
            }
            Self::List(vs) => {
                trace!("encode list length");
                let len = vs
                    .len()
                    .try_into()
                    .context("list length does not fit in u64")?;
                leb128::write::unsigned(&mut payload.writer(), len)
                    .context("failed to encode list length")?;
                let mut txs = Vec::with_capacity(vs.len());
                for v in vs {
                    trace!("encode list element");
                    let v = v.encode(payload).await?;
                    txs.push(v)
                }
                Ok(txs
                    .iter()
                    .any(Option::is_some)
                    .then_some(AsyncTransmission::List(txs)))
            }
            Self::Record(vs) => {
                trace!("encode record");
                let mut txs = Vec::with_capacity(vs.len());
                for v in vs {
                    trace!("encode record field");
                    let v = v.encode(payload).await?;
                    txs.push(v)
                }
                Ok(txs
                    .iter()
                    .any(Option::is_some)
                    .then_some(AsyncTransmission::Record(txs)))
            }
            Self::Tuple(vs) => {
                trace!("encode tuple");
                let mut txs = Vec::with_capacity(vs.len());
                for v in vs {
                    trace!("encode tuple element");
                    let v = v.encode(payload).await?;
                    txs.push(v)
                }
                Ok(txs
                    .iter()
                    .any(Option::is_some)
                    .then_some(AsyncTransmission::Tuple(txs)))
            }
            Self::Variant {
                discriminant,
                nested: None,
            } => {
                trace!("encode variant discriminant");
                leb128::write::unsigned(&mut payload.writer(), discriminant.into())
                    .context("failed to encode variant discriminant")?;
                Ok(None)
            }
            Self::Variant {
                discriminant,
                nested: Some(v),
            } => {
                trace!("encode variant discriminant");
                leb128::write::unsigned(&mut payload.writer(), discriminant.into())
                    .context("failed to encode variant discriminant")?;
                trace!("encode variant value");
                let tx = v.encode(payload).await?;
                Ok(tx.map(Box::new).map(|nested| AsyncTransmission::Variant {
                    discriminant,
                    nested,
                }))
            }
            Self::Enum(v) => {
                trace!(v, "encode enum");
                leb128::write::unsigned(&mut payload.writer(), v.into())
                    .context("failed to encode enum")?;
                Ok(None)
            }
            Self::Option(None) => {
                trace!("encode option::none");
                payload.put_u8(0);
                Ok(None)
            }
            Self::Option(Some(v)) => {
                trace!("encode option::some");
                payload.put_u8(1);
                let tx = v.encode(payload).await?;
                Ok(tx.map(Box::new).map(AsyncTransmission::Option))
            }
            Self::Result(Ok(None)) => {
                trace!("encode empty result::ok");
                payload.put_u8(0);
                Ok(None)
            }
            Self::Result(Ok(Some(v))) => {
                trace!("encode nested result::ok");
                payload.put_u8(0);
                let tx = v.encode(payload).await?;
                Ok(tx.map(Box::new).map(AsyncTransmission::ResultOk))
            }
            Self::Result(Err(None)) => {
                trace!("encode empty result::err");
                payload.put_u8(1);
                Ok(None)
            }
            Self::Result(Err(Some(v))) => {
                trace!("encode nested result::err");
                payload.put_u8(1);
                let tx = v.encode(payload).await?;
                Ok(tx.map(Box::new).map(AsyncTransmission::ResultErr))
            }
            Self::Flags(v) => {
                trace!(v, "encode flags");
                leb128::write::unsigned(&mut payload.writer(), v)
                    .context("failed to encode flags")?;
                Ok(None)
            }
            Self::Future(mut v) => {
                trace!("encode future");
                if let Some(v) = poll_immediate(&mut v).await {
                    trace!("encode ready future value");
                    payload.put_u8(1);
                    if let Some(v) = v.context("failed to acquire value of the future")? {
                        v.encode(payload).await
                    } else {
                        Ok(None)
                    }
                } else {
                    trace!("encode pending future value");
                    payload.put_u8(0);
                    Ok(Some(AsyncTransmission::Future(v)))
                }
            }
            Self::Stream(v) => {
                trace!("encode stream");
                trace!("encode pending stream value");
                // TODO: Use `poll_immediate` to check if the stream has finished and encode if it
                // has - buffer otherwise
                payload.put_u8(0);
                Ok(Some(AsyncTransmission::Stream(v)))
            }
        }
    }
}

pub struct TupleReceiver<'a, const N: usize, T> {
    rx: T,
    nested: Option<[Option<AsyncSubscription<T>>; N]>,
    types: [&'a Type; N],
    payload: Bytes,
}

impl<const N: usize, S> TupleReceiver<'_, N, S>
where
    S: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
{
    #[instrument(level = "trace", skip_all)]
    pub async fn receive(mut self) -> anyhow::Result<([Value; N], Box<dyn Buf + Send>, S)> {
        trace!("receive tuple");
        let mut vals = Vec::with_capacity(N);
        let mut payload: Box<dyn Buf + Send> = Box::new(self.payload);
        for (i, ty) in self.types.iter().enumerate() {
            trace!(i, "receive tuple element");
            let v;
            (v, payload) = Value::receive_context(
                ty,
                payload,
                &mut self.rx,
                self.nested
                    .as_mut()
                    .map(|nested| nested.get_mut(i).map(Option::take).flatten())
                    .flatten(),
            )
            .await
            .context("failed to receive tuple value")?;
            vals.push(v);
        }
        let vals = if let Ok(vals) = vals.try_into() {
            vals
        } else {
            bail!("invalid value vector received")
        };
        Ok((vals, payload, self.rx))
    }
}

pub struct TupleDynamicReceiver<'a, S> {
    rx: S,
    nested: Vec<Option<AsyncSubscription<S>>>,
    types: &'a [Type],
    payload: Bytes,
}

impl<S> TupleDynamicReceiver<'_, S>
where
    S: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
{
    #[instrument(level = "trace", skip_all)]
    pub async fn receive(mut self) -> anyhow::Result<(Vec<Value>, Box<dyn Buf + Send>, S)> {
        trace!("receive tuple");
        let mut vals = Vec::with_capacity(self.types.len());
        let mut payload: Box<dyn Buf + Send> = Box::new(self.payload);
        for (i, ty) in self.types.iter().enumerate() {
            trace!(i, "receive tuple element");
            let v;
            (v, payload) = Value::receive_context(
                ty,
                payload,
                &mut self.rx,
                self.nested.get_mut(i).map(Option::take).flatten(),
            )
            .await
            .context("failed to receive tuple value")?;
            vals.push(v);
        }
        Ok((vals, payload, self.rx))
    }
}

#[async_trait]
pub trait Acceptor {
    type Subject;
    type Transmitter: Transmitter<Subject = Self::Subject> + Send + Sync + 'static;

    async fn accept(
        self,
        subject: Self::Subject,
    ) -> anyhow::Result<(Self::Subject, Self::Transmitter)>;
}

#[async_trait]
pub trait Invocation {
    type Transmission: Future<Output = anyhow::Result<()>> + Send + 'static;
    type TransmissionFailed: Future<Output = ()> + Send + 'static;

    async fn invoke<T, U>(
        self,
        instance: &str,
        name: &str,
        params: T,
    ) -> anyhow::Result<(Self::Transmission, Self::TransmissionFailed)>
    where
        T: IntoIterator<Item = U> + Send,
        T::IntoIter: ExactSizeIterator<Item = U> + Send,
        U: Encode<U> + Send + 'static;
}

#[async_trait]
pub trait Client: Sync {
    type Subject: Subject + Clone + Send + 'static;
    type Transmission: Future<Output = anyhow::Result<()>> + Send + 'static;
    type Subscriber: Subscriber<
            Subject = Self::Subject,
            SubscribeError = anyhow::Error,
            StreamError = anyhow::Error,
        > + Send
        + Sync
        + 'static;
    type Acceptor: Acceptor<Subject = Self::Subject> + Send + 'static;
    type InvocationStream: Stream<Item = anyhow::Result<(Bytes, Self::Subject, Self::Subscriber, Self::Acceptor)>>
        + Unpin
        + Send
        + 'static;
    type Invocation: Invocation<Transmission = Self::Transmission> + Send;

    async fn serve(&self, instance: &str, name: &str) -> anyhow::Result<Self::InvocationStream>;

    #[instrument(level = "trace", skip(self, params))]
    async fn serve_dynamic(
        &self,
        instance: &str,
        name: &str,
        params: Arc<[Type]>,
    ) -> anyhow::Result<
        Pin<
            Box<
                dyn Stream<
                        Item = anyhow::Result<(
                            Vec<Value>,
                            Self::Subject,
                            <Self::Acceptor as Acceptor>::Transmitter,
                        )>,
                    > + Send,
            >,
        >,
    > {
        let invocations = self.serve(instance, name).await?;
        Ok(Box::pin(invocations.and_then({
            move |(payload, rx, sub, accept)| {
                let params = Arc::clone(&params);
                async move {
                    let sub = sub
                        .subscribe_tuple_dynamic(rx.clone(), &params, payload)
                        .await
                        .context("failed to subscribe for parameters")?;
                    let (tx_subject, tx) = accept
                        .accept(rx)
                        .await
                        .context("failed to accept invocation")?;
                    let (params, _, _) = sub
                        .receive()
                        .await
                        .context("failed to receive parameters")?;
                    Ok((params, tx_subject, tx))
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
    );

    #[instrument(level = "trace", skip(self, params, results))]
    async fn invoke_dynamic<T>(
        &self,
        instance: &str,
        name: &str,
        params: T,
        results: &[Type],
    ) -> anyhow::Result<(Vec<Value>, Self::Transmission)>
    where
        T: IntoIterator<Item = Value> + Send,
        T::IntoIter: ExactSizeIterator<Item = Value> + Send,
    {
        let (inv, sub, result_rx, error_rx) = self.new_invocation();

        let (results, mut error) = try_join!(
            async {
                sub.subscribe_tuple_dynamic(result_rx, results, Bytes::default())
                    .await
                    .context("failed to subscribe on result subjects")
            },
            async {
                sub.subscribe(error_rx)
                    .await
                    .context("failed to subscribe on error subject")
            },
        )?;

        let (tx, tx_fail) = inv
            .invoke(instance, name, params)
            .await
            .context("failed to invoke function")?;

        select! {
            _ = tx_fail => {
                trace!("transmission task failed");
                match tx.await {

                    Err(err) => bail!(anyhow!(err).context("transmission failed")),
                    Ok(_) => bail!("transmission task desynchronisation occured"),
                }
            }
            vals = results.receive() => {
                trace!("received results");
                let (vals, _, _) = vals?;
                return Ok((vals, tx))
            }
            err = error.next() => {
                let err = err.context("error stream unexpectedly finished")?.context("failed to receive error")?;
                trace!("received error");
                let (err, _) = String::receive_sync(err, &mut error).await.context("failed to receive error string")?;
                bail!(err)
            }
        }
    }
}
