use core::array;
use core::borrow::Borrow;
use core::fmt::Debug;
use core::future::{ready, Future};
use core::iter::zip;
use core::pin::{pin, Pin};
use core::task::{self, Poll};
use core::time::Duration;

use std::borrow::Cow;
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

pub type IncomingInputStream = Box<dyn Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin>;

#[async_trait]
pub trait Transmitter: Sync {
    type Subject: Subject + Send + Sync + Clone;
    type PublishError: Error + Send + Sync + 'static;

    fn transmit(
        &self,
        subject: Self::Subject,
        payload: Bytes,
    ) -> impl Future<Output = Result<(), Self::PublishError>> + Send;

    #[instrument(level = "trace", ret, skip_all)]
    fn transmit_static(
        &self,
        subject: Self::Subject,
        payload: impl Encode,
    ) -> impl Future<Output = anyhow::Result<()>> + Send {
        let mut buf = BytesMut::default();
        async move {
            let tx = payload
                .encode(&mut buf)
                .await
                .context("failed to encode value")?;
            try_join!(
                async {
                    if let Some(tx) = tx {
                        self.transmit_async(subject.clone(), tx)
                            .await
                            .context("failed to transmit asynchronous value")?;
                    }
                    Ok(())
                },
                async {
                    trace!("encode tuple value");
                    self.transmit(subject.clone(), buf.freeze())
                        .await
                        .context("failed to transmit value")
                },
            )?;
            Ok(())
        }
    }

    #[instrument(level = "trace", ret, skip_all)]
    fn transmit_tuple_dynamic<T>(
        &self,
        subject: Self::Subject,
        values: T,
    ) -> impl Future<Output = anyhow::Result<()>> + Send
    where
        T: IntoIterator<Item = Value> + Send,
        T::IntoIter: ExactSizeIterator<Item = Value> + Send,
    {
        let values = values.into_iter();
        let mut buf = BytesMut::new();
        let mut nested = Vec::with_capacity(values.len());
        async {
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
                        self.transmit_async(subject, v).await.with_context(|| {
                            format!("failed to transmit asynchronous tuple element {i}")
                        })
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
    }

    #[instrument(level = "trace", ret, skip_all)]
    async fn transmit_async(
        &self,
        subject: Self::Subject,
        value: AsyncValue,
    ) -> anyhow::Result<()> {
        match value {
            AsyncValue::List(nested) | AsyncValue::Record(nested) | AsyncValue::Tuple(nested) => {
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
            AsyncValue::Variant {
                discriminant,
                nested,
            } => {
                trace!(discriminant, "transmit asynchronous variant value");
                self.transmit_async(subject.child(Some(discriminant.into())), *nested)
                    .await
            }
            AsyncValue::Option(nested) => {
                trace!("transmit asynchronous option value");
                self.transmit_async(subject.child(Some(1)), *nested)
                    .await
                    .context("failed to transmit asynchronous `option::some` value")
            }
            AsyncValue::ResultOk(nested) => {
                trace!("transmit asynchronous result::ok value");
                self.transmit_async(subject.child(Some(0)), *nested)
                    .await
                    .context("failed to transmit asynchronous `result::ok` value")
            }
            AsyncValue::ResultErr(nested) => {
                trace!("transmit asynchronous result::err value");
                self.transmit_async(subject.child(Some(1)), *nested)
                    .await
                    .context("failed to transmit asynchronous `result::err` value")
            }
            AsyncValue::Future(v) => {
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
            AsyncValue::Stream(mut v) => {
                let mut i: u64 = 0;
                loop {
                    let item = v.try_next().await.context("failed to receive item")?;
                    match item {
                        None => {
                            self.transmit(subject, Bytes::from_static(&[0])).await?;
                            return Ok(());
                        }
                        Some(vs) => {
                            let mut payload = BytesMut::new();
                            let len = vs
                                .len()
                                .try_into()
                                .context("stream batch length does not fit in u32")?;
                            trace!(len, "encode stream item batch length");
                            leb128::write::unsigned(&mut (&mut payload).writer(), len)
                                .context("failed to encode stream item batch length")?;
                            let mut txs = Vec::with_capacity(vs.len());
                            for v in vs {
                                trace!("encode stream batch item");
                                if let Some(v) = v {
                                    let tx = v
                                        .encode(&mut payload)
                                        .await
                                        .context("failed to encode stream element value")?;
                                    txs.push(tx);
                                } else {
                                    txs.push(None);
                                }
                            }
                            let txs: FuturesUnordered<_> = zip(0.., txs)
                                .filter_map(|(i, v)| {
                                    let v = v?;
                                    Some((i, v))
                                })
                                .map(|(i, v)| {
                                    trace!(i, "transmit nested asynchronous stream element value");
                                    let subject = subject.child(Some(i));
                                    Box::pin(async move {
                                        self.transmit_async(subject, v).await.with_context(|| {
                                             format!("failed to transmit nested asynchronous stream element value {i}")
                                        })
                                    })
                                })
                                .collect();
                            let payload = payload.freeze();
                            try_join!(
                                async {
                                    try_join_all(txs).await.context(
                                        "failed to transmit nested asynchronous stream element values",
                                    )
                                },
                                async {
                                    self.transmit(subject.clone(), payload)
                                        .await
                                        .context("failed to transmit stream element value")
                                },
                            )?;
                            i = i.saturating_add(len);
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
        nested: Option<Box<AsyncSubscription<T>>>,
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
    pub fn try_unwrap_stream(self) -> anyhow::Result<(T, Option<AsyncSubscriptionDemux<T>>)> {
        match self {
            AsyncSubscription::Stream { subscriber, nested } => {
                let nested = nested.map(|sub| sub.demux()).transpose()?;
                Ok((subscriber, nested))
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
        _cx: &mut task::Context<'_>,
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
    pub fn select(&mut self, _i: u64) -> AsyncSubscription<DemuxStream> {
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
    fn child(&self, i: Option<u64>) -> Self;
}

/// Defines how nested async value subscriptions are to be established.
pub trait Subscribe: Sized {
    fn subscribe<T: Subscriber + Send + Sync>(
        _subscriber: &T,
        _subject: T::Subject,
    ) -> impl Future<Output = Result<Option<AsyncSubscription<T::Stream>>, T::SubscribeError>> + Send
    {
        async { Ok(None) }
    }
}

impl Subscribe for bool {}
impl Subscribe for u8 {}
impl Subscribe for u16 {}
impl Subscribe for u32 {}
impl Subscribe for u64 {}
impl Subscribe for i8 {}
impl Subscribe for i16 {}
impl Subscribe for i32 {}
impl Subscribe for i64 {}
impl Subscribe for f32 {}
impl Subscribe for f64 {}
impl Subscribe for char {}
impl Subscribe for &str {}
impl Subscribe for String {}
impl Subscribe for Duration {}
impl Subscribe for Bytes {}
impl Subscribe for anyhow::Error {}
impl Subscribe for () {}

impl<A> Subscribe for Option<A>
where
    A: Subscribe,
{
    #[instrument(level = "trace", skip_all)]
    async fn subscribe<T: Subscriber + Send + Sync>(
        subscriber: &T,
        subject: T::Subject,
    ) -> Result<Option<AsyncSubscription<T::Stream>>, T::SubscribeError> {
        let a = A::subscribe(subscriber, subject.child(Some(1))).await?;
        Ok(a.map(Box::new).map(AsyncSubscription::Option))
    }
}

impl<A> Subscribe for &[A]
where
    A: Subscribe,
{
    #[instrument(level = "trace", skip_all)]
    async fn subscribe<T: Subscriber + Send + Sync>(
        subscriber: &T,
        subject: T::Subject,
    ) -> Result<Option<AsyncSubscription<T::Stream>>, T::SubscribeError> {
        let a = A::subscribe(subscriber, subject.child(None)).await?;
        Ok(a.map(Box::new).map(AsyncSubscription::List))
    }
}

impl<A> Subscribe for Vec<A>
where
    A: Subscribe,
{
    #[instrument(level = "trace", skip_all)]
    async fn subscribe<T: Subscriber + Send + Sync>(
        subscriber: &T,
        subject: T::Subject,
    ) -> Result<Option<AsyncSubscription<T::Stream>>, T::SubscribeError> {
        <&[A]>::subscribe(subscriber, subject).await
    }
}

impl<A> Subscribe for &Vec<A>
where
    A: Subscribe,
{
    #[instrument(level = "trace", skip_all)]
    async fn subscribe<T: Subscriber + Send + Sync>(
        subscriber: &T,
        subject: T::Subject,
    ) -> Result<Option<AsyncSubscription<T::Stream>>, T::SubscribeError> {
        <&[A]>::subscribe(subscriber, subject).await
    }
}

impl<O, E> Subscribe for Result<O, E>
where
    O: Subscribe,
    E: Subscribe,
{
    #[instrument(level = "trace", skip_all)]
    async fn subscribe<T: Subscriber + Send + Sync>(
        subscriber: &T,
        subject: T::Subject,
    ) -> Result<Option<AsyncSubscription<T::Stream>>, T::SubscribeError> {
        let (ok, err) = try_join!(
            O::subscribe(subscriber, subject.child(Some(0))),
            E::subscribe(subscriber, subject.child(Some(1))),
        )?;
        Ok(
            (ok.is_some() || err.is_some()).then_some(AsyncSubscription::Result {
                ok: ok.map(Box::new),
                err: err.map(Box::new),
            }),
        )
    }
}

impl<A> Subscribe for (A,)
where
    A: Subscribe,
{
    #[instrument(level = "trace", skip_all)]
    async fn subscribe<T: Subscriber + Send + Sync>(
        subscriber: &T,
        subject: T::Subject,
    ) -> Result<Option<AsyncSubscription<T::Stream>>, T::SubscribeError> {
        let a = A::subscribe(subscriber, subject.child(Some(0))).await?;
        Ok(a.is_some().then(|| AsyncSubscription::Tuple(vec![a])))
    }
}

macro_rules! impl_subscribe_tuple {
    ($n:expr; $($t:tt),+; $($i:tt),+) => {
        impl<$($t),+> Subscribe for ($($t),+)
        where
            $($t: Subscribe),+
        {
            #[instrument(level = "trace", skip_all)]
            async fn subscribe<T: Subscriber + Send + Sync>(
                subscriber: &T,
                subject: T::Subject,
            ) -> Result<Option<AsyncSubscription<T::Stream>>, T::SubscribeError> {
                trace!(len = $n, "subscribe for tuple");
                let subs = try_join!($($t::subscribe(subscriber, subject.child(Some($i)))),+)?;
                let subs = [$(subs.$i),+];
                Ok(subs
                    .iter()
                    .any(Option::is_some)
                    .then(|| AsyncSubscription::Tuple(subs.into())))
            }
        }
    };
}

impl_subscribe_tuple!(2; A, B; 0, 1);
impl_subscribe_tuple!(3; A, B, C; 0, 1, 2);
impl_subscribe_tuple!(4; A, B, C, D; 0, 1, 2, 3);
impl_subscribe_tuple!(5; A, B, C, D, E; 0, 1, 2, 3, 4);
impl_subscribe_tuple!(6; A, B, C, D, E, F; 0, 1, 2, 3, 4, 5);
impl_subscribe_tuple!(7; A, B, C, D, E, F, G; 0, 1, 2, 3, 4, 5, 6);
impl_subscribe_tuple!(8; A, B, C, D, E, F, G, H; 0, 1, 2, 3, 4, 5, 6, 7);
impl_subscribe_tuple!(9; A, B, C, D, E, F, G, H, I; 0, 1, 2, 3, 4, 5, 6, 7, 8);
impl_subscribe_tuple!(10; A, B, C, D, E, F, G, H, I, J; 0, 1, 2, 3, 4, 5, 6, 7, 8, 9);
impl_subscribe_tuple!(11; A, B, C, D, E, F, G, H, I, J, K; 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10);
impl_subscribe_tuple!(12; A, B, C, D, E, F, G, H, I, J, K, L; 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11);
impl_subscribe_tuple!(13; A, B, C, D, E, F, G, H, I, J, K, L, M; 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12);
impl_subscribe_tuple!(14; A, B, C, D, E, F, G, H, I, J, K, L, M, N; 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13);
impl_subscribe_tuple!(15; A, B, C, D, E, F, G, H, I, J, K, L, M, N, O; 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14);
impl_subscribe_tuple!(16; A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P; 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15);

impl<E> Subscribe for Box<dyn Stream<Item = anyhow::Result<Vec<E>>> + Send + Sync + Unpin>
where
    E: Subscribe,
{
    #[instrument(level = "trace", skip_all)]
    async fn subscribe<T: Subscriber + Send + Sync>(
        subscriber: &T,
        subject: T::Subject,
    ) -> Result<Option<AsyncSubscription<T::Stream>>, T::SubscribeError> {
        let nested = subject.child(None);
        let (subscriber, nested) = try_join!(
            subscriber.subscribe(subject),
            E::subscribe(subscriber, nested),
        )?;
        let nested = nested.map(Box::new);
        Ok(Some(AsyncSubscription::Stream { subscriber, nested }))
    }
}

impl Subscribe for IncomingInputStream {
    #[instrument(level = "trace", skip_all)]
    async fn subscribe<T: Subscriber + Send + Sync>(
        subscriber: &T,
        subject: T::Subject,
    ) -> Result<Option<AsyncSubscription<T::Stream>>, T::SubscribeError> {
        let subscriber = subscriber.subscribe(subject).await?;
        Ok(Some(AsyncSubscription::Stream {
            subscriber,
            nested: None,
        }))
    }
}

#[async_trait]
pub trait Subscriber: Sync {
    type Subject: Subject + Send + Sync + Clone;
    type Stream: Stream<Item = Result<Bytes, Self::StreamError>> + Send + Sync + Unpin;
    type SubscribeError: Send;
    type StreamError: Send;

    fn subscribe(
        &self,
        subject: Self::Subject,
    ) -> impl Future<Output = Result<Self::Stream, Self::SubscribeError>> + Send;

    #[instrument(level = "trace", skip_all)]
    fn subscribe_async(
        &self,
        subject: Self::Subject,
        ty: impl Borrow<Type> + Send,
    ) -> impl Future<Output = Result<Option<AsyncSubscription<Self::Stream>>, Self::SubscribeError>> + Send
    {
        // pin to break cycle in the compiler
        Box::pin(async {
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
                | Type::F32
                | Type::F64
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
                        .subscribe_async_sized_optional(
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
                Type::Stream(None) => {
                    let subscriber = self.subscribe(subject).await?;
                    Ok(Some(AsyncSubscription::Stream {
                        subscriber,
                        nested: None,
                    }))
                }
                Type::Stream(Some(ty)) => {
                    let nested = subject.child(None);
                    let (subscriber, nested) = try_join!(
                        self.subscribe(subject),
                        self.subscribe_async(nested, ty.as_ref())
                    )?;
                    let nested = nested.map(Box::new);
                    Ok(Some(AsyncSubscription::Stream { subscriber, nested }))
                }
                Type::Resource(Resource::Pollable) => {
                    self.subscribe_async(subject, &Type::Future(None)).await
                }
                Type::Resource(Resource::InputStream) => {
                    self.subscribe_async(subject, &Type::Stream(Some(Arc::new(Type::U8))))
                        .await
                }
                Type::Resource(Resource::OutputStream | Resource::Dynamic(..)) => Ok(None),
            }
        })
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
    async fn subscribe_async_sized<const N: usize>(
        &self,
        subject: impl Borrow<Self::Subject> + Send,
        types: [impl Borrow<Type> + Send + Sync; N],
    ) -> Result<Option<[Option<AsyncSubscription<Self::Stream>>; N]>, Self::SubscribeError> {
        self.subscribe_async_sized_optional(subject, types.map(Some))
            .await
    }

    #[instrument(level = "trace", skip_all)]
    async fn subscribe_async_sized_optional<const N: usize>(
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
    async fn subscribe_tuple(
        &self,
        subject: Self::Subject,
        types: impl IntoIterator<Item = impl Borrow<Type> + Send> + Send,
    ) -> Result<Option<AsyncSubscription<Self::Stream>>, Self::SubscribeError> {
        let sub = self.subscribe_async_iter(subject, types).await?;
        Ok(sub.map(AsyncSubscription::Tuple))
    }
}

/// A tuple of a dynamic size
pub struct DynamicTuple<T>(pub Vec<T>);

#[async_trait]
impl<T> Encode for DynamicTuple<T>
where
    T: Encode + Send,
{
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        trace!("encode tuple");
        let txs = encode_sized_iter(payload, self.0).await?;
        Ok(txs.map(AsyncValue::Tuple))
    }
}

/// Iterator wrapper
pub struct ListIter<T, I>(pub I)
where
    T: Encode,
    I: IntoIterator<Item = T> + Send,
    I::IntoIter: ExactSizeIterator + Send;

#[async_trait]
impl<T, I> Encode for ListIter<T, I>
where
    T: Encode,
    I: IntoIterator<Item = T> + Send,
    I::IntoIter: ExactSizeIterator + Send,
{
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        mut payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        let it = self.0.into_iter();
        trace!(len = it.len(), "encode list length");
        let len = it
            .len()
            .try_into()
            .context("list length does not fit in u64")?;
        leb128::write::unsigned(&mut (&mut payload).writer(), len)
            .context("failed to encode list length")?;
        let txs = encode_sized_iter(payload, it).await?;
        Ok(txs.map(AsyncValue::List))
    }
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
    F32(f32),
    F64(f64),
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
    Stream(Pin<Box<dyn Stream<Item = anyhow::Result<Vec<Option<Value>>>> + Send>>),
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}

impl From<u8> for Value {
    fn from(v: u8) -> Self {
        Self::U8(v)
    }
}

impl From<u16> for Value {
    fn from(v: u16) -> Self {
        Self::U16(v)
    }
}

impl From<u32> for Value {
    fn from(v: u32) -> Self {
        Self::U32(v)
    }
}

impl From<u64> for Value {
    fn from(v: u64) -> Self {
        Self::U64(v)
    }
}

impl From<i8> for Value {
    fn from(v: i8) -> Self {
        Self::S8(v)
    }
}

impl From<i16> for Value {
    fn from(v: i16) -> Self {
        Self::S16(v)
    }
}

impl From<i32> for Value {
    fn from(v: i32) -> Self {
        Self::S32(v)
    }
}

impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Self::S64(v)
    }
}

impl From<f32> for Value {
    fn from(v: f32) -> Self {
        Self::F32(v)
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Self::F64(v)
    }
}

impl From<char> for Value {
    fn from(v: char) -> Self {
        Self::Char(v)
    }
}

impl From<String> for Value {
    fn from(v: String) -> Self {
        Self::String(v)
    }
}

impl From<Vec<Value>> for Value {
    fn from(v: Vec<Value>) -> Self {
        Self::List(v)
    }
}

impl From<Option<Value>> for Value {
    fn from(v: Option<Value>) -> Self {
        Self::Option(v.map(Box::new))
    }
}

impl From<Bytes> for Value {
    fn from(v: Bytes) -> Self {
        Self::List(v.into_iter().map(Value::U8).collect())
    }
}

impl From<(Value,)> for Value {
    fn from((a,): (Value,)) -> Self {
        Self::Tuple(vec![a])
    }
}

impl From<(Value, Value)> for Value {
    fn from((a, b): (Value, Value)) -> Self {
        Self::Tuple(vec![a, b])
    }
}

struct StreamValue<T> {
    items: ReceiverStream<anyhow::Result<T>>,
    producer: JoinHandle<()>,
}

impl<T> Stream for StreamValue<T> {
    type Item = anyhow::Result<T>;

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

pub enum AsyncValue {
    List(Vec<Option<AsyncValue>>),
    Record(Vec<Option<AsyncValue>>),
    Tuple(Vec<Option<AsyncValue>>),
    Variant {
        discriminant: u32,
        nested: Box<AsyncValue>,
    },
    Option(Box<AsyncValue>),
    ResultOk(Box<AsyncValue>),
    ResultErr(Box<AsyncValue>),
    Future(Pin<Box<dyn Future<Output = anyhow::Result<Option<Value>>> + Send>>),
    Stream(Pin<Box<dyn Stream<Item = anyhow::Result<Vec<Option<Value>>>> + Send>>),
}

fn map_tuple_subscription<T>(
    sub: Option<AsyncSubscription<T>>,
) -> anyhow::Result<Vec<Option<AsyncSubscription<T>>>> {
    let sub = sub.map(AsyncSubscription::try_unwrap_tuple).transpose()?;
    Ok(sub.unwrap_or_default())
}

/// Receive bytes until `payload` contains at least `n` bytes
#[instrument(level = "trace", skip(payload, rx))]
pub async fn receive_at_least<'a>(
    payload: impl Buf + Send + 'a,
    rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
    n: usize,
) -> anyhow::Result<Box<dyn Buf + Send + 'a>> {
    let mut payload: Box<dyn Buf + Send + 'a> = Box::new(payload);
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
    let mut payload: Box<dyn Buf + Send + 'a> = Box::new(payload);
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
pub async fn receive_leb128_signed<'a>(
    payload: impl Buf + Send + 'a,
    rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
) -> anyhow::Result<(i64, Box<dyn Buf + Send + 'a>)> {
    let mut payload: Box<dyn Buf + Send + 'a> = Box::new(payload);
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
pub async fn receive_list_header<'a>(
    payload: impl Buf + Send + 'a,
    rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
) -> anyhow::Result<(u32, Box<dyn Buf + Send + 'a>)> {
    trace!("decode list length");
    let (len, payload) = receive_leb128_unsigned(payload, rx)
        .await
        .context("failed to decode list length")?;
    let len = len.try_into().context("list length does not fit in u32")?;
    Ok((len, payload))
}

#[instrument(level = "trace", skip_all)]
pub async fn receive_discriminant<'a>(
    payload: impl Buf + Send + 'a,
    rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
) -> anyhow::Result<(u32, Box<dyn Buf + Send + 'a>)> {
    let (discriminant, payload) = receive_leb128_unsigned(payload, rx)
        .await
        .context("failed to decode discriminant")?;
    let discriminant = discriminant
        .try_into()
        .context("discriminant does not fit in u32")?;
    Ok((discriminant, payload))
}

#[async_trait]
pub trait Receive<'a>: Sized {
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static;

    async fn receive_sync(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)> {
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
pub trait ReceiveContext<'a, Ctx>: Sized + Send
where
    Ctx: ?Sized + Send + Sync + 'static,
{
    async fn receive_context<T>(
        cx: &Ctx,
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static;

    #[instrument(level = "trace", skip_all)]
    fn receive_context_sync(
        cx: &'a Ctx,
        payload: impl Buf + Send + 'a,
        rx: &'a mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'a),
    ) -> impl Future<Output = anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>> + Send + 'a
    where
        Self: 'a,
    {
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
    }

    /// Receive<'a> a list
    #[instrument(level = "trace", skip_all)]
    fn receive_list_context<T>(
        cx: &Ctx,
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> impl Future<Output = anyhow::Result<(Vec<Self>, Box<dyn Buf + Send + 'a>)>> + Send
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
    {
        async {
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
    }

    /// Receive<'a> a tuple
    #[instrument(level = "trace", skip_all)]
    async fn receive_tuple_context<T, I>(
        cx: I,
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Vec<Self>, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
        I: IntoIterator<Item = &'a Ctx> + Send,
        I::IntoIter: ExactSizeIterator + Send,
    {
        let mut sub = sub.map(AsyncSubscription::try_unwrap_tuple).transpose()?;
        let cx = cx.into_iter();
        trace!(len = cx.len(), "decode tuple");
        let mut els = Vec::with_capacity(cx.len());
        let mut payload: Box<dyn Buf + Send + 'a> = Box::new(payload);
        for (i, cx) in cx.enumerate() {
            trace!(i, "decode tuple element");
            let sub = sub
                .as_mut()
                .and_then(|sub| sub.get_mut(i).and_then(Option::take));
            let el;
            (el, payload) = Self::receive_context(cx, payload, rx, sub)
                .await
                .with_context(|| format!("failed to decode value of tuple element {i}"))?;
            els.push(el);
        }
        Ok((els, payload))
    }

    #[instrument(level = "trace", skip_all)]
    fn receive_stream_item_context<T>(
        cx: Option<&Ctx>,
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: &mut Option<AsyncSubscriptionDemux<T>>,
        offset: u64,
    ) -> impl Future<Output = anyhow::Result<(Option<Vec<Option<Self>>>, Box<dyn Buf + Send + 'a>)>>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
    {
        async {
            trace!("decode stream length");
            let (len, mut payload) = receive_leb128_unsigned(payload, rx).await?;
            match len {
                0 => Ok((None, payload)),
                len => {
                    let cap = len
                        .try_into()
                        .context("stream element length does not fit in usize")?;
                    trace!(len, "decode stream elements");
                    if let Some(cx) = cx {
                        let mut els = Vec::with_capacity(cap);
                        let end = offset
                            .checked_add(len)
                            .context("index would overflow u64")?;
                        for i in offset..end {
                            let sub = sub.as_mut().map(|sub| sub.select(i));
                            trace!(i, "decode stream element");
                            let el;
                            (el, payload) = Self::receive_context(cx, payload, rx, sub)
                                .await
                                .with_context(|| {
                                    format!("failed to decode value of stream element {i}")
                                })?;
                            els.push(Some(el));
                        }
                        Ok((Some(els), payload))
                    } else {
                        Ok((Some((0..cap).map(|_| None).collect()), payload))
                    }
                }
            }
        }
    }
}

#[instrument(level = "trace", skip_all)]
pub async fn receive_stream_item<'a, E, T>(
    payload: impl Buf + Send + 'a,
    rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
    sub: &mut Option<AsyncSubscriptionDemux<T>>,
    offset: u64,
) -> anyhow::Result<(Option<Vec<E>>, Box<dyn Buf + Send + 'a>)>
where
    E: Receive<'a>,
    T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
{
    let (len, mut payload) = receive_leb128_unsigned(payload, rx).await?;
    match len {
        0 => Ok((None, payload)),
        len => {
            let cap = len
                .try_into()
                .context("stream element length does not fit in usize")?;
            trace!(len, "decode stream elements");
            let mut els = Vec::with_capacity(cap);
            let end = offset
                .checked_add(len)
                .context("index would overflow u64")?;
            for i in offset..end {
                let sub = sub.as_mut().map(|sub| sub.select(i));
                trace!(i, "decode stream element");
                let el;
                (el, payload) = E::receive(payload, rx, sub)
                    .await
                    .with_context(|| format!("failed to decode value of stream element {i}"))?;
                els.push(el);
            }
            Ok((Some(els), payload))
        }
    }
}

#[async_trait]
impl<'a, R, Ctx> ReceiveContext<'a, Ctx> for R
where
    R: Receive<'a> + Send,
    Ctx: Send + Sync + 'static,
{
    #[instrument(level = "trace", skip_all)]
    async fn receive_context<T>(
        _cx: &Ctx,
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
    {
        R::receive(payload, rx, sub).await
    }
}

#[async_trait]
impl<'a> Receive<'a> for bool {
    #[instrument(level = "trace", skip_all, fields(ty = "bool"))]
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
    {
        let mut payload = receive_at_least(payload, rx, 1).await?;
        Ok((payload.get_u8() == 1, payload))
    }
}

#[async_trait]
impl<'a> Receive<'a> for u8 {
    #[instrument(level = "trace", skip_all, fields(ty = "u8"))]
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
    {
        let mut payload = receive_at_least(payload, rx, 1).await?;
        Ok((payload.get_u8(), payload))
    }
}

#[async_trait]
impl<'a> Receive<'a> for u16 {
    #[instrument(level = "trace", skip_all, fields(ty = "u16"))]
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
    {
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
impl<'a> Receive<'a> for u32 {
    #[instrument(level = "trace", skip_all, fields(ty = "u32"))]
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
    {
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
impl<'a> Receive<'a> for u64 {
    #[instrument(level = "trace", skip_all, fields(ty = "u64"))]
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
    {
        let (v, payload) = receive_leb128_unsigned(payload, rx)
            .await
            .context("failed to decode u64")?;
        Ok((v, payload))
    }
}

#[async_trait]
impl<'a> Receive<'a> for i8 {
    #[instrument(level = "trace", skip_all, fields(ty = "i8"))]
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
    {
        let mut payload = receive_at_least(payload, rx, 1).await?;
        Ok((payload.get_i8(), payload))
    }
}

#[async_trait]
impl<'a> Receive<'a> for i16 {
    #[instrument(level = "trace", skip_all, fields(ty = "i16"))]
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
    {
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
impl<'a> Receive<'a> for i32 {
    #[instrument(level = "trace", skip_all, fields(ty = "i32"))]
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
    {
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
impl<'a> Receive<'a> for i64 {
    #[instrument(level = "trace", skip_all, fields(ty = "i64"))]
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
    {
        let (v, payload) = receive_leb128_signed(payload, rx)
            .await
            .context("failed to decode s64")?;
        Ok((v, payload))
    }
}

#[async_trait]
impl<'a> Receive<'a> for f32 {
    #[instrument(level = "trace", skip_all, fields(ty = "f32"))]
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
    {
        let mut payload = receive_at_least(payload, rx, 8).await?;
        Ok((payload.get_f32_le(), payload))
    }
}

#[async_trait]
impl<'a> Receive<'a> for f64 {
    #[instrument(level = "trace", skip_all, fields(ty = "f64"))]
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
    {
        let mut payload = receive_at_least(payload, rx, 8).await?;
        Ok((payload.get_f64_le(), payload))
    }
}

#[async_trait]
impl<'a> Receive<'a> for char {
    #[instrument(level = "trace", skip_all, fields(ty = "char"))]
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
    {
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
impl<'a> Receive<'a> for String {
    #[instrument(level = "trace", skip_all, fields(ty = "String"))]
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
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
impl<'a> Receive<'a> for Duration {
    #[instrument(level = "trace", skip_all, fields(ty = "duration"))]
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
    {
        let (v, payload) = u64::receive_sync(payload, rx).await?;
        Ok((Duration::from_nanos(v), payload))
    }
}

#[async_trait]
impl<'a, E> Receive<'a> for Vec<E>
where
    E: Receive<'a> + Send,
{
    #[instrument(level = "trace", skip_all, fields(ty = "vec"))]
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
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
impl<'a> Receive<'a> for Bytes {
    #[instrument(level = "trace", skip_all, fields(ty = "bytes"))]
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
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
impl<'a, E> Receive<'a> for Option<E>
where
    E: Receive<'a>,
{
    #[instrument(level = "trace", skip_all, fields(ty = "option"))]
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
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
impl<'a, Ok, Err> Receive<'a> for Result<Ok, Err>
where
    Ok: Receive<'a>,
    Err: Receive<'a>,
{
    #[instrument(level = "trace", skip_all, fields(ty = "result"))]
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
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
impl<'a> Receive<'a> for anyhow::Error {
    #[instrument(level = "trace", skip_all, fields(ty = "anyhow::Error"))]
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
    {
        let (err, payload) = String::receive(payload, rx, sub).await?;
        Ok((anyhow!(err), payload))
    }
}

#[async_trait]
impl<'a, E> Receive<'a> for Pin<Box<dyn Future<Output = anyhow::Result<E>> + Send>>
where
    E: Receive<'a> + Send + 'static,
{
    #[instrument(level = "trace", skip_all, fields(ty = "future"))]
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
    {
        let Some((subscriber, sub)) = sub.map(AsyncSubscription::try_unwrap_future).transpose()?
        else {
            bail!("future subscription type mismatch")
        };
        trace!("decode future");
        let mut payload = receive_at_least(payload, rx, 1).await?;
        trace!("decode future variant");
        match payload.get_u8() {
            0 => {
                trace!("decode pending future");
                Ok((
                    Box::pin(async move {
                        trace!("decode future nested value");
                        let (v, _) =
                            E::receive(Bytes::default(), &mut pin!(subscriber), sub).await?;
                        Ok(v)
                    }),
                    payload,
                ))
            }
            1 => {
                trace!("decode ready future nested value");
                let (v, payload) = E::receive(payload, rx, sub).await?;
                Ok((Box::pin(ready(Ok(v))), payload))
            }
            _ => bail!("invalid `future` variant"),
        }
    }
}

#[async_trait]
impl<'a, E> Receive<'a> for Box<dyn Stream<Item = anyhow::Result<Vec<E>>> + Send + Sync + Unpin>
where
    E: Receive<'a> + Send + Sync + 'static,
{
    #[instrument(level = "trace", skip_all, fields(ty = "stream"))]
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
    {
        let Some((subscriber, mut sub)) =
            sub.map(AsyncSubscription::try_unwrap_stream).transpose()?
        else {
            bail!("stream subscription type mismatch")
        };
        trace!("decode stream");
        let mut payload = receive_at_least(payload, rx, 1).await?;
        match payload.get_u8() {
            0 => {
                let (items_tx, items_rx) = mpsc::channel(1);
                let producer = spawn(async move {
                    let mut payload: Box<dyn Buf + Send + 'a> = Box::new(Bytes::new());
                    let mut i = 0;
                    let mut subscriber = pin!(subscriber);
                    loop {
                        match receive_stream_item(payload, &mut subscriber, &mut sub, i).await {
                            Ok((Some(vs), buf)) => {
                                payload = buf;
                                i = i.saturating_add(vs.len() as _);
                                if let Err(err) = items_tx.send(Ok(vs)).await {
                                    trace!(?err, "item receiver closed");
                                    return;
                                }
                            }
                            Ok((None, _)) => {
                                trace!("stream end received, close stream");
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
                    Box::new(StreamValue {
                        producer,
                        items: ReceiverStream::new(items_rx),
                    }),
                    payload,
                ))
            }
            1 => {
                let (len, mut payload) = receive_leb128_unsigned(payload, rx).await?;
                trace!(len, "decode stream elements");
                let cap = len
                    .try_into()
                    .context("stream element length does not fit in usize")?;
                let mut els = Vec::with_capacity(cap);
                for i in 0..len {
                    trace!(i, "decode stream element");
                    let sub = sub.as_mut().map(|sub| sub.select(i));
                    let el;
                    (el, payload) = E::receive(payload, rx, sub)
                        .await
                        .with_context(|| format!("failed to decode value of list element {i}"))?;
                    els.push(el);
                }
                if els.is_empty() {
                    Ok((Box::new(stream::empty()), payload))
                } else {
                    Ok((Box::new(stream::iter([Ok(els)])), payload))
                }
            }
            _ => bail!("invalid stream readiness byte"),
        }
    }
}

#[async_trait]
impl<'a> Receive<'a> for IncomingInputStream {
    #[instrument(level = "trace", skip_all, fields(ty = "IncomingInputStream"))]
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
    {
        let Some((subscriber, _sub)) = sub.map(AsyncSubscription::try_unwrap_stream).transpose()?
        else {
            bail!("stream subscription type mismatch")
        };
        trace!("decode stream");
        let mut payload = receive_at_least(payload, rx, 1).await?;
        match payload.get_u8() {
            0 => {
                let (items_tx, items_rx) = mpsc::channel(1);
                let producer = spawn(async move {
                    let mut payload: Box<dyn Buf + Send + 'a> = Box::new(Bytes::new());
                    let mut subscriber = pin!(subscriber);
                    loop {
                        match Bytes::receive_sync(payload, &mut subscriber).await {
                            Ok((vs, _)) if vs.is_empty() => {
                                trace!("stream end received, close stream");
                                return;
                            }
                            Ok((vs, buf)) => {
                                payload = buf;
                                if let Err(err) = items_tx.send(Ok(vs)).await {
                                    trace!(?err, "item receiver closed");
                                    return;
                                }
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
                    Box::new(StreamValue {
                        producer,
                        items: ReceiverStream::new(items_rx),
                    }),
                    payload,
                ))
            }
            1 => {
                let (buf, payload) = Bytes::receive_sync(payload, rx)
                    .await
                    .context("failed to receive bytes")?;
                if buf.is_empty() {
                    Ok((Box::new(stream::empty()), payload))
                } else {
                    Ok((Box::new(stream::iter([Ok(buf)])), payload))
                }
            }
            _ => bail!("invalid stream readiness byte"),
        }
    }
}

#[async_trait]
impl<'a> Receive<'a> for () {
    #[instrument(level = "trace", skip_all, fields(ty = "IncomingInputStream"))]
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        _rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        _sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
    {
        Ok(((), Box::new(payload)))
    }
}

#[async_trait]
impl<'a, A> Receive<'a> for (A,)
where
    A: Receive<'a>,
{
    #[instrument(level = "trace", skip_all)]
    async fn receive<T>(
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
    {
        trace!(i = 0, "decode tuple element");
        let mut sub = map_tuple_subscription(sub)?;
        let (a, payload) = A::receive(payload, rx, sub.get_mut(0).and_then(Option::take)).await?;
        Ok(((a,), payload))
    }
}

macro_rules! impl_receive_tuple {
    ($n:expr; $($t:tt),+; $($i:tt),+) => {
        #[async_trait]
        impl<'a, $($t),+> Receive<'a> for ($($t),+)
        where
            $($t: Receive<'a> + Send),+
        {
            #[instrument(level = "trace", skip_all)]
            async fn receive<T>(
                payload: impl Buf + Send + 'a,
                rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
                sub: Option<AsyncSubscription<T>>,
            ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
            where
                T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
            {
                trace!(len = $n, "receive tuple");
                let mut sub = sub.map(AsyncSubscription::try_unwrap_tuple).transpose()?;
                let mut payload: Box<dyn Buf + Send + 'a> = Box::new(payload);
                let vals = ($(
                    {
                        let v;
                        trace!(i = $i, "decode tuple element");
                        (v, payload) = $t::receive(payload, rx, sub.as_mut().and_then(|sub| sub.get_mut($i)).and_then(Option::take)).await.context(concat!("failed to receive tuple element ", $i))?;
                        v
                    }
                ),+);
                Ok((vals, payload))
            }
        }
    };
}

impl_receive_tuple!(2; A, B; 0, 1);
impl_receive_tuple!(3; A, B, C; 0, 1, 2);
impl_receive_tuple!(4; A, B, C, D; 0, 1, 2, 3);
impl_receive_tuple!(5; A, B, C, D, E; 0, 1, 2, 3, 4);
impl_receive_tuple!(6; A, B, C, D, E, F; 0, 1, 2, 3, 4, 5);
impl_receive_tuple!(7; A, B, C, D, E, F, G; 0, 1, 2, 3, 4, 5, 6);
impl_receive_tuple!(8; A, B, C, D, E, F, G, H; 0, 1, 2, 3, 4, 5, 6, 7);
impl_receive_tuple!(9; A, B, C, D, E, F, G, H, I; 0, 1, 2, 3, 4, 5, 6, 7, 8);
impl_receive_tuple!(10; A, B, C, D, E, F, G, H, I, J; 0, 1, 2, 3, 4, 5, 6, 7, 8, 9);
impl_receive_tuple!(11; A, B, C, D, E, F, G, H, I, J, K; 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10);
impl_receive_tuple!(12; A, B, C, D, E, F, G, H, I, J, K, L; 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11);
impl_receive_tuple!(13; A, B, C, D, E, F, G, H, I, J, K, L, M; 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12);
impl_receive_tuple!(14; A, B, C, D, E, F, G, H, I, J, K, L, M, N; 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13);
impl_receive_tuple!(15; A, B, C, D, E, F, G, H, I, J, K, L, M, N, O; 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14);
impl_receive_tuple!(16; A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P; 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15);

#[async_trait]
impl<'a> ReceiveContext<'a, Type> for Value {
    #[instrument(level = "trace", skip_all)]
    async fn receive_context<T>(
        ty: &Type,
        payload: impl Buf + Send + 'a,
        rx: &mut (impl Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin),
        sub: Option<AsyncSubscription<T>>,
    ) -> anyhow::Result<(Self, Box<dyn Buf + Send + 'a>)>
    where
        T: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + 'static,
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
            Type::F32 => {
                let (v, payload) = f32::receive(payload, rx, sub).await?;
                Ok((Self::F32(v), payload))
            }
            Type::F64 => {
                let (v, payload) = f64::receive(payload, rx, sub).await?;
                Ok((Self::F64(v), payload))
            }
            Type::Char => {
                let (v, payload) = char::receive(payload, rx, sub).await?;
                Ok((Self::Char(v), payload))
            }
            Type::String => {
                let (v, payload) = String::receive(payload, rx, sub).await?;
                Ok((Self::String(v), payload))
            }
            Type::List(ty) => {
                let (els, payload) = Self::receive_list_context(ty, payload, rx, sub).await?;
                Ok((Self::List(els), payload))
            }
            Type::Record(tys) => {
                let mut fields = Vec::with_capacity(tys.len());
                let mut sub = sub.map(AsyncSubscription::try_unwrap_record).transpose()?;
                let mut payload: Box<dyn Buf + Send + 'a> = Box::new(payload);
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
                let mut payload: Box<dyn Buf + Send + 'a> = Box::new(payload);
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
                let Some((subscriber, nested)) =
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
                                let buf = pin!(subscriber)
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
                                    &mut pin!(subscriber),
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
            Type::Stream(ty) => {
                let Some((subscriber, mut sub)) =
                    sub.map(AsyncSubscription::try_unwrap_stream).transpose()?
                else {
                    bail!("stream subscription type mismatch")
                };
                trace!("decode stream length");
                let (len, mut payload) = receive_leb128_unsigned(payload, rx).await?;
                match len {
                    0 => {
                        let (items_tx, items_rx) = mpsc::channel(1);
                        let ty = ty.clone();
                        let producer = spawn(async move {
                            let mut payload: Box<dyn Buf + Send + 'a> = Box::new(Bytes::new());
                            let mut i = 0;
                            let mut subscriber = pin!(subscriber);
                            loop {
                                match Self::receive_stream_item_context::<T>(
                                    ty.as_deref(),
                                    payload,
                                    &mut subscriber,
                                    &mut sub,
                                    i,
                                )
                                .await
                                {
                                    Ok((Some(vs), buf)) => {
                                        payload = buf;
                                        i = i.saturating_add(vs.len() as _);
                                        if let Err(err) = items_tx.send(Ok(vs)).await {
                                            trace!(?err, "item receiver closed");
                                            return;
                                        }
                                    }
                                    Ok((None, _)) => {
                                        trace!("stream end received, close stream");
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
                    len => {
                        let cap = len
                            .try_into()
                            .context("stream element length does not fit in usize")?;
                        trace!(len, "decode stream elements");
                        let els = if let Some(ty) = ty {
                            let mut els = Vec::with_capacity(cap);
                            for i in 0..len {
                                trace!(i, "decode stream element");
                                let sub = sub.as_mut().map(|sub| sub.select(i));
                                let el;
                                (el, payload) = Self::receive_context(ty, payload, rx, sub)
                                    .await
                                    .with_context(|| {
                                    format!("failed to decode value of list element {i}")
                                })?;
                                els.push(Some(el));
                            }
                            els
                        } else {
                            (0..cap).map(|_| None).collect()
                        };
                        Ok((
                            Value::Stream(Box::pin(stream::once(async { Ok(els) }))),
                            payload,
                        ))
                    }
                }
            }
            Type::Resource(Resource::Pollable) => {
                Self::receive_context(&Type::Future(None), payload, rx, sub).await
            }
            Type::Resource(Resource::InputStream) => {
                Self::receive_context(&Type::Stream(Some(Arc::new(Type::U8))), payload, rx, sub)
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
pub trait Encode: Send {
    async fn encode(self, payload: &mut (impl BufMut + Send))
        -> anyhow::Result<Option<AsyncValue>>;
}

#[async_trait]
impl<'a, 'b, T> Encode for &'a &'b T
where
    T: Sync + ?Sized,
    &'b T: Encode,
{
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        (*self).encode(payload).await
    }
}

#[async_trait]
impl Encode for () {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        _payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        Ok(None)
    }
}

#[async_trait]
impl Encode for bool {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        trace!(v = self, "encode bool");
        payload.put_u8(if self { 1 } else { 0 });
        Ok(None)
    }
}

#[async_trait]
impl Encode for u8 {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        trace!(v = self, "encode u8");
        payload.put_u8(self);
        Ok(None)
    }
}

#[async_trait]
impl Encode for u16 {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        trace!(v = self, "encode u16");
        leb128::write::unsigned(&mut payload.writer(), self.into())
            .context("failed to encode u16")?;
        Ok(None)
    }
}

#[async_trait]
impl Encode for u32 {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        trace!(v = self, "encode u32");
        leb128::write::unsigned(&mut payload.writer(), self.into())
            .context("failed to encode u32")?;
        Ok(None)
    }
}

#[async_trait]
impl Encode for u64 {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        trace!(v = self, "encode u64");
        leb128::write::unsigned(&mut payload.writer(), self).context("failed to encode u64")?;
        Ok(None)
    }
}

#[async_trait]
impl Encode for i8 {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        trace!(v = self, "encode s8");
        payload.put_i8(self);
        Ok(None)
    }
}

#[async_trait]
impl Encode for i16 {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        trace!(v = self, "encode s16");
        leb128::write::signed(&mut payload.writer(), self.into())
            .context("failed to encode s16")?;
        Ok(None)
    }
}

#[async_trait]
impl Encode for i32 {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        trace!(v = self, "encode s32");
        leb128::write::signed(&mut payload.writer(), self.into())
            .context("failed to encode s32")?;
        Ok(None)
    }
}

#[async_trait]
impl Encode for i64 {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        trace!(v = self, "encode s64");
        leb128::write::signed(&mut payload.writer(), self).context("failed to encode s64")?;
        Ok(None)
    }
}

#[async_trait]
impl Encode for f32 {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        trace!(v = self, "encode float32");
        payload.put_f32_le(self);
        Ok(None)
    }
}

#[async_trait]
impl Encode for f64 {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        trace!(v = self, "encode float64");
        payload.put_f64_le(self);
        Ok(None)
    }
}

#[async_trait]
impl Encode for char {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        trace!(v = ?self, "encode char");
        leb128::write::unsigned(&mut payload.writer(), self.into())
            .context("failed to encode char")?;
        Ok(None)
    }
}

#[async_trait]
impl Encode for Duration {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        trace!(v = ?self, "encode duration");
        let v: u64 = self
            .as_nanos()
            .try_into()
            .context("duration nanosecond count does not fit in u64")?;
        v.encode(payload).await
    }
}

macro_rules! impl_encode_copy_ref {
    ($t:ty) => {
        #[async_trait]
        impl Encode for &$t {
            #[instrument(level = "trace", skip_all)]
            async fn encode(
                self,
                payload: &mut (impl BufMut + Send),
            ) -> anyhow::Result<Option<AsyncValue>> {
                (*self).encode(payload).await
            }
        }
    };
}

impl_encode_copy_ref!(());
impl_encode_copy_ref!(bool);
impl_encode_copy_ref!(u8);
impl_encode_copy_ref!(u16);
impl_encode_copy_ref!(u32);
impl_encode_copy_ref!(u64);
impl_encode_copy_ref!(i8);
impl_encode_copy_ref!(i16);
impl_encode_copy_ref!(i32);
impl_encode_copy_ref!(i64);
impl_encode_copy_ref!(f32);
impl_encode_copy_ref!(f64);
impl_encode_copy_ref!(char);
impl_encode_copy_ref!(Duration);

#[async_trait]
impl Encode for &str {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        trace!(len = self.len(), "encode string length");
        let len = self
            .len()
            .try_into()
            .context("string length does not fit in u64")?;
        leb128::write::unsigned(&mut payload.writer(), len)
            .context("failed to encode string length")?;
        trace!(self, "encode string value");
        payload.put_slice(self.as_bytes());
        Ok(None)
    }
}

#[async_trait]
impl Encode for String {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        self.as_str().encode(payload).await
    }
}

#[async_trait]
impl Encode for &String {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        self.as_str().encode(payload).await
    }
}

#[async_trait]
impl Encode for Cow<'_, str> {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        self.as_ref().encode(payload).await
    }
}

#[async_trait]
impl Encode for &Cow<'_, str> {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        self.as_ref().encode(payload).await
    }
}

#[async_trait]
impl Encode for Bytes {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        mut payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        trace!(len = self.len(), "encode byte list length");
        let len = self
            .len()
            .try_into()
            .context("byte list length does not fit in u64")?;
        leb128::write::unsigned(&mut (&mut payload).writer(), len)
            .context("failed to encode byte list length")?;
        payload.put(self);
        Ok(None)
    }
}

#[async_trait]
impl Encode for &Bytes {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        self.clone().encode(payload).await
    }
}

#[async_trait]
impl Encode for anyhow::Error {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        (&self).encode(payload).await
    }
}

#[async_trait]
impl Encode for &anyhow::Error {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        format!("{self:#}").encode(payload).await
    }
}

#[async_trait]
impl<T> Encode for Arc<T>
where
    T: Encode + Sync,
{
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        let v = Arc::into_inner(self).context("failed to unwrap Arc")?;
        v.encode(payload).await
    }
}

#[async_trait]
impl<T> Encode for Option<T>
where
    T: Encode,
{
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        match self {
            None => {
                trace!("encode option::none");
                payload.put_u8(0);
                Ok(None)
            }
            Some(v) => {
                trace!("encode option::some");
                payload.put_u8(1);
                let tx = v.encode(payload).await?;
                Ok(tx.map(Box::new).map(AsyncValue::Option))
            }
        }
    }
}

#[async_trait]
impl<'a, T> Encode for &'a Option<T>
where
    T: Sync,
    &'a T: Encode,
{
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        match self {
            None => {
                trace!("encode option::none");
                payload.put_u8(0);
                Ok(None)
            }
            Some(v) => {
                trace!("encode option::some");
                payload.put_u8(1);
                let tx = v.encode(payload).await?;
                Ok(tx.map(Box::new).map(AsyncValue::Option))
            }
        }
    }
}

#[async_trait]
impl<T, U> Encode for Result<T, U>
where
    T: Encode,
    U: Encode,
{
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        match self {
            Ok(v) => {
                trace!("encode nested result::ok");
                payload.put_u8(0);
                let tx = v.encode(payload).await?;
                Ok(tx.map(Box::new).map(AsyncValue::ResultOk))
            }
            Err(v) => {
                trace!("encode nested result::err");
                payload.put_u8(1);
                let tx = v.encode(payload).await?;
                Ok(tx.map(Box::new).map(AsyncValue::ResultErr))
            }
        }
    }
}

#[async_trait]
impl<'a, T, U> Encode for &'a Result<T, U>
where
    T: Sync,
    U: Sync,
    &'a T: Encode,
    &'a U: Encode,
{
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        match self {
            Ok(v) => {
                trace!("encode nested result::ok");
                payload.put_u8(0);
                let tx = v.encode(payload).await?;
                Ok(tx.map(Box::new).map(AsyncValue::ResultOk))
            }
            Err(v) => {
                trace!("encode nested result::err");
                payload.put_u8(1);
                let tx = v.encode(payload).await?;
                Ok(tx.map(Box::new).map(AsyncValue::ResultErr))
            }
        }
    }
}

#[async_trait]
impl<const N: usize, T> Encode for [T; N]
where
    T: Encode + Sync,
{
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        trace!(len = N, "encode list length");
        let len = self
            .len()
            .try_into()
            .context("list length does not fit in u64")?;
        leb128::write::unsigned(&mut payload.writer(), len)
            .context("failed to encode list length")?;
        let mut txs = Vec::with_capacity(self.len());
        for v in self {
            trace!("encode list element");
            let v = v.encode(payload).await?;
            txs.push(v)
        }
        Ok(txs
            .iter()
            .any(Option::is_some)
            .then_some(AsyncValue::List(txs)))
    }
}

#[async_trait]
impl<'a, const N: usize, T> Encode for &'a [T; N]
where
    T: Sync,
    &'a T: Encode,
{
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        self.as_slice().encode(payload).await
    }
}

#[async_trait]
impl<'a, T> Encode for &'a [T]
where
    T: Sync,
    &'a T: Encode,
{
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        trace!(len = self.len(), "encode list length");
        let len = self
            .len()
            .try_into()
            .context("list length does not fit in u64")?;
        leb128::write::unsigned(&mut payload.writer(), len)
            .context("failed to encode list length")?;
        let mut txs = Vec::with_capacity(self.len());
        for v in self {
            trace!("encode list element");
            let v = v.encode(payload).await?;
            txs.push(v)
        }
        Ok(txs
            .iter()
            .any(Option::is_some)
            .then_some(AsyncValue::List(txs)))
    }
}

#[async_trait]
impl<T> Encode for Vec<T>
where
    T: Encode,
{
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        trace!(len = self.len(), "encode list length");
        let len = self
            .len()
            .try_into()
            .context("list length does not fit in u64")?;
        leb128::write::unsigned(&mut payload.writer(), len)
            .context("failed to encode list length")?;
        let mut txs = Vec::with_capacity(self.len());
        for v in self {
            trace!("encode list element");
            let v = v.encode(payload).await?;
            txs.push(v)
        }
        Ok(txs
            .iter()
            .any(Option::is_some)
            .then_some(AsyncValue::List(txs)))
    }
}

#[async_trait]
impl<'a, T> Encode for &'a Vec<T>
where
    T: Sync,
    &'a T: Encode,
{
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        self.as_slice().encode(payload).await
    }
}

#[async_trait]
impl<A> Encode for (A,)
where
    A: Encode,
{
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        trace!("encode 1 element tuple");
        let (a,) = self;
        let a = a.encode(payload).await?;
        Ok(a.is_some().then(|| AsyncValue::Tuple(vec![a])))
    }
}

#[async_trait]
impl<'a, A> Encode for &'a (A,)
where
    A: Sync,
    &'a A: Encode,
{
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        trace!("encode 1 element tuple");
        let (a,) = self;
        let a = a.encode(payload).await?;
        Ok(a.is_some().then(|| AsyncValue::Tuple(vec![a])))
    }
}

macro_rules! impl_encode_tuple {
    ($n:expr; $($t:tt),+; $($i:tt),+) => {
        #[async_trait]
        impl<$($t),+> Encode for ($($t),+)
        where
            $($t: Encode),+
        {
            #[instrument(level = "trace", skip_all)]
            async fn encode(
                self,
                payload: &mut (impl BufMut + Send),
            ) -> anyhow::Result<Option<AsyncValue>> {
                trace!(len = $n, "encode tuple");
                let txs = ($(self.$i.encode(payload).await?),+);
                let txs = [$(txs.$i),+];
                Ok(txs
                    .iter()
                    .any(Option::is_some)
                    .then(|| AsyncValue::Tuple(txs.into())))
            }
        }

        #[async_trait]
        impl<'a, $($t),+> Encode for &'a ($($t),+)
        where
            $(
                $t: Sync,
                &'a $t: Encode,
            )+
        {
            #[instrument(level = "trace", skip_all)]
            async fn encode(
                self,
                payload: &mut (impl BufMut + Send),
            ) -> anyhow::Result<Option<AsyncValue>> {
                trace!(len = $n, "encode tuple");
                let txs = ($(self.$i.encode(payload).await?),+);
                let txs = [$(txs.$i),+];
                Ok(txs
                    .iter()
                    .any(Option::is_some)
                    .then(|| AsyncValue::Tuple(txs.into())))
            }
        }
    };
}

impl_encode_tuple!(2; A, B; 0, 1);
impl_encode_tuple!(3; A, B, C; 0, 1, 2);
impl_encode_tuple!(4; A, B, C, D; 0, 1, 2, 3);
impl_encode_tuple!(5; A, B, C, D, E; 0, 1, 2, 3, 4);
impl_encode_tuple!(6; A, B, C, D, E, F; 0, 1, 2, 3, 4, 5);
impl_encode_tuple!(7; A, B, C, D, E, F, G; 0, 1, 2, 3, 4, 5, 6);
impl_encode_tuple!(8; A, B, C, D, E, F, G, H; 0, 1, 2, 3, 4, 5, 6, 7);
impl_encode_tuple!(9; A, B, C, D, E, F, G, H, I; 0, 1, 2, 3, 4, 5, 6, 7, 8);
impl_encode_tuple!(10; A, B, C, D, E, F, G, H, I, J; 0, 1, 2, 3, 4, 5, 6, 7, 8, 9);
impl_encode_tuple!(11; A, B, C, D, E, F, G, H, I, J, K; 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10);
impl_encode_tuple!(12; A, B, C, D, E, F, G, H, I, J, K, L; 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11);
impl_encode_tuple!(13; A, B, C, D, E, F, G, H, I, J, K, L, M; 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12);
impl_encode_tuple!(14; A, B, C, D, E, F, G, H, I, J, K, L, M, N; 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13);
impl_encode_tuple!(15; A, B, C, D, E, F, G, H, I, J, K, L, M, N, O; 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14);
impl_encode_tuple!(16; A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P; 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15);

#[instrument(level = "trace", skip(payload))]
pub fn encode_discriminant(payload: impl BufMut, discriminant: u32) -> anyhow::Result<()> {
    trace!("encode discriminant");
    leb128::write::unsigned(&mut payload.writer(), discriminant.into())
        .context("failed to encode discriminant")?;
    Ok(())
}

#[instrument(level = "trace", skip_all)]
pub async fn encode_sized_iter<T, U>(
    mut payload: impl BufMut + Send,
    it: T,
) -> anyhow::Result<Option<Vec<Option<AsyncValue>>>>
where
    T: IntoIterator<Item = U>,
    T::IntoIter: ExactSizeIterator<Item = U>,
    U: Encode,
{
    trace!("encode statically-sized iterator");
    let it = it.into_iter();
    let mut txs = Vec::with_capacity(it.len());
    for v in it {
        trace!("encode iterator element");
        let v = v.encode(&mut payload).await?;
        txs.push(v)
    }
    Ok(txs.iter().any(Option::is_some).then_some(txs))
}

#[async_trait]
impl Encode for Value {
    #[instrument(level = "trace", skip_all)]
    async fn encode(
        self,
        mut payload: &mut (impl BufMut + Send),
    ) -> anyhow::Result<Option<AsyncValue>> {
        match self {
            Self::Bool(v) => v.encode(payload).await,
            Self::U8(v) => v.encode(payload).await,
            Self::U16(v) => v.encode(payload).await,
            Self::U32(v) => v.encode(payload).await,
            Self::U64(v) => v.encode(payload).await,
            Self::S8(v) => v.encode(payload).await,
            Self::S16(v) => v.encode(payload).await,
            Self::S32(v) => v.encode(payload).await,
            Self::S64(v) => v.encode(payload).await,
            Self::F32(v) => v.encode(payload).await,
            Self::F64(v) => v.encode(payload).await,
            Self::Char(v) => v.encode(payload).await,
            Self::String(v) => v.encode(payload).await,
            Self::List(v) => v.encode(payload).await,
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
                    .then_some(AsyncValue::Record(txs)))
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
                    .then_some(AsyncValue::Tuple(txs)))
            }
            Self::Variant {
                discriminant,
                nested: None,
            } => {
                encode_discriminant(payload, discriminant)?;
                Ok(None)
            }
            Self::Variant {
                discriminant,
                nested: Some(v),
            } => {
                encode_discriminant(&mut payload, discriminant)?;
                trace!("encode variant value");
                let tx = v.encode(payload).await?;
                Ok(tx.map(Box::new).map(|nested| AsyncValue::Variant {
                    discriminant,
                    nested,
                }))
            }
            Self::Enum(v) => {
                trace!("encode enum");
                encode_discriminant(payload, v)?;
                Ok(None)
            }
            Self::Option(None) => None::<()>.encode(payload).await,
            Self::Option(Some(v)) => Some(*v).encode(payload).await,
            Self::Result(Ok(None)) => Ok::<(), ()>(()).encode(payload).await,
            Self::Result(Ok(Some(v))) => Ok::<Value, ()>(*v).encode(payload).await,
            Self::Result(Err(None)) => Err::<(), ()>(()).encode(payload).await,
            Self::Result(Err(Some(v))) => Err::<(), Value>(*v).encode(payload).await,
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
                    Ok(Some(AsyncValue::Future(v)))
                }
            }
            Self::Stream(v) => {
                trace!("encode stream");
                trace!("encode pending stream value");
                // TODO: Use `poll_immediate` to check if the stream has finished and encode if it
                // has - buffer otherwise
                payload.put_u8(0);
                Ok(Some(AsyncValue::Stream(v)))
            }
        }
    }
}

pub struct TupleReceiverSized<'a, const N: usize, T> {
    rx: T,
    nested: Option<[Option<AsyncSubscription<T>>; N]>,
    types: [&'a Type; N],
}

impl<const N: usize, S> TupleReceiverSized<'_, N, S>
where
    S: Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin + 'static,
{
    #[instrument(level = "trace", skip_all)]
    pub async fn receive(
        mut self,
        payload: impl Buf + Send + 'static,
    ) -> anyhow::Result<([Value; N], Box<dyn Buf + Send>, S)> {
        trace!("receive tuple");
        let mut vals = Vec::with_capacity(N);
        let mut payload: Box<dyn Buf + Send> = Box::new(payload);
        for (i, ty) in self.types.iter().enumerate() {
            trace!(i, "receive tuple element");
            let v;
            (v, payload) = Value::receive_context(
                ty,
                payload,
                &mut self.rx,
                self.nested
                    .as_mut()
                    .and_then(|nested| nested.get_mut(i).and_then(Option::take)),
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

pub trait Acceptor {
    type Subject;
    type Transmitter: Transmitter<Subject = Self::Subject> + Send + Sync + 'static;

    fn accept(
        self,
        subject: Self::Subject,
    ) -> impl Future<Output = anyhow::Result<(Self::Subject, Self::Subject, Self::Transmitter)>> + Send;
}

pub trait Invocation {
    type Transmission: Future<Output = anyhow::Result<()>> + Send + 'static;
    type TransmissionFailed: Future<Output = ()> + Send + 'static;

    fn invoke(
        self,
        instance: &str,
        name: &str,
        params: impl Encode,
    ) -> impl Future<Output = anyhow::Result<(Self::Transmission, Self::TransmissionFailed)>> + Send;
}

/// Invocation received from a peer
pub struct IncomingInvocation<Ctx, Sub: Subscriber, Acc: Acceptor> {
    pub context: Ctx,
    pub payload: Bytes,
    pub param_subject: Acc::Subject,
    pub error_subject: Acc::Subject,
    pub handshake_subject: Sub::Subject,
    pub subscriber: Sub,
    pub acceptor: Acc,
}

impl<Ctx, Sub, Acc> IncomingInvocation<Ctx, Sub, Acc>
where
    Acc: Acceptor,
    Acc::Subject: Clone,
    Sub: Subscriber<
            Subject = Acc::Subject,
            SubscribeError = anyhow::Error,
            StreamError = anyhow::Error,
        > + Send,
    Sub::Stream: 'static,
{
    /// Accept with statically-typed parameter type `T`
    #[instrument(level = "trace", skip_all)]
    pub async fn accept_static<'a, T: Receive<'a> + Subscribe + 'static>(
        self,
    ) -> anyhow::Result<AcceptedInvocation<Ctx, T, <Acc as Acceptor>::Transmitter>> {
        let Self {
            context,
            payload,
            param_subject,
            error_subject,
            handshake_subject,
            subscriber,
            acceptor,
        } = self;
        // TODO: Subscribe for errors
        let (mut rx, nested, errors) = try_join!(
            subscriber.subscribe(param_subject.clone()),
            T::subscribe(&subscriber, param_subject.clone()),
            subscriber.subscribe(error_subject),
        )
        .context("failed to subscribe for parameters")?;
        // TODO: Propagate the error stream
        _ = errors;
        let (result_subject, error_subject, transmitter) = acceptor
            .accept(handshake_subject)
            .await
            .context("failed to accept invocation")?;
        let (params, _) = T::receive(payload, &mut rx, nested)
            .await
            .context("failed to receive parameters")?;
        Ok(AcceptedInvocation {
            context,
            params,
            result_subject,
            error_subject,
            transmitter,
        })
    }

    /// Accept with dynamically-typed parameter type `params`
    #[instrument(level = "trace", skip_all)]
    pub async fn accept_dynamic(
        self,
        params: Arc<[Type]>,
    ) -> anyhow::Result<AcceptedInvocation<Ctx, Vec<Value>, <Acc as Acceptor>::Transmitter>> {
        let Self {
            context,
            payload,
            param_subject,
            error_subject,
            handshake_subject,
            subscriber,
            acceptor,
        } = self;
        let (mut rx, nested, errors) = try_join!(
            subscriber.subscribe(param_subject.clone()),
            subscriber.subscribe_tuple(param_subject.clone(), params.as_ref()),
            subscriber.subscribe(error_subject),
        )
        .context("failed to subscribe for parameters")?;
        let (result_subject, error_subject, transmitter) = acceptor
            .accept(handshake_subject)
            .await
            .context("failed to accept invocation")?;
        // TODO: Propagate the error stream
        _ = errors;
        let (params, _) =
            ReceiveContext::receive_tuple_context(params.as_ref(), payload, &mut rx, nested)
                .await
                .context("failed to receive parameters")?;
        Ok(AcceptedInvocation {
            context,
            params,
            result_subject,
            error_subject,
            transmitter,
        })
    }
}

/// An accepted invocation received from a peer
pub struct AcceptedInvocation<Ctx, T, Tx: Transmitter> {
    pub context: Ctx,
    pub params: T,
    pub result_subject: Tx::Subject,
    pub error_subject: Tx::Subject,
    pub transmitter: Tx,
}

impl<Ctx, T, Tx: Transmitter> AcceptedInvocation<Ctx, T, Tx> {
    /// Map `[Self::context]` to another type
    pub fn map_context<U>(self, f: impl FnOnce(Ctx) -> U) -> AcceptedInvocation<U, T, Tx> {
        let Self {
            context,
            params,
            result_subject,
            error_subject,
            transmitter,
        } = self;
        AcceptedInvocation {
            context: f(context),
            params,
            result_subject,
            error_subject,
            transmitter,
        }
    }

    /// Map `[Self::params]` to another type
    pub fn map_params<U>(self, f: impl FnOnce(T) -> U) -> AcceptedInvocation<Ctx, U, Tx> {
        let Self {
            context,
            params,
            result_subject,
            error_subject,
            transmitter,
        } = self;
        AcceptedInvocation {
            context,
            params: f(params),
            result_subject,
            error_subject,
            transmitter,
        }
    }

    /// Map `[Self::transmitter]`, `[Self::result_subject]` and `[Self::error_subject]` to another type
    pub fn map_transmitter<U: Transmitter>(
        self,
        f: impl FnOnce(Tx, Tx::Subject, Tx::Subject) -> (U, U::Subject, U::Subject),
    ) -> AcceptedInvocation<Ctx, T, U> {
        let Self {
            context,
            params,
            result_subject,
            error_subject,
            transmitter,
        } = self;
        let (transmitter, result_subject, error_subject) =
            f(transmitter, result_subject, error_subject);
        AcceptedInvocation {
            context,
            params,
            result_subject,
            error_subject,
            transmitter,
        }
    }
}

/// Invocation ready to be transmitted to a peer
pub struct OutgoingInvocation<Invocation, Subscriber, Subject> {
    pub invocation: Invocation,
    pub subscriber: Subscriber,
    pub result_subject: Subject,
    pub error_subject: Subject,
}

/// wRPC transmission client used to invoke and serve functions
pub trait Client: Sync {
    type Context: Send + 'static;
    type Subject: Subject + Clone + Send + Sync + 'static;
    type Transmission: Future<Output = anyhow::Result<()>> + Send + 'static;
    type Subscriber: Subscriber<
            Subject = Self::Subject,
            SubscribeError = anyhow::Error,
            StreamError = anyhow::Error,
        > + Send
        + Sync
        + 'static;
    type Acceptor: Acceptor<Subject = Self::Subject> + Send + 'static;
    type Invocation: Invocation<Transmission = Self::Transmission> + Send;
    type InvocationStream<Ctx, T, Tx: Transmitter>: Stream<Item = anyhow::Result<AcceptedInvocation<Ctx, T, Tx>>>
        + Send;

    /// Serve function `name` from instance `instance`
    fn serve<Ctx, T, Tx, S, Fut>(
        &self,
        instance: &str,
        name: &str,
        svc: S,
    ) -> impl Future<Output = anyhow::Result<Self::InvocationStream<Ctx, T, Tx>>> + Send
    where
        Tx: Transmitter,
        S: tower::Service<
                IncomingInvocation<Self::Context, Self::Subscriber, Self::Acceptor>,
                Future = Fut,
            > + Send
            + Clone
            + 'static,
        Fut: Future<Output = Result<AcceptedInvocation<Ctx, T, Tx>, anyhow::Error>> + Send;

    /// Serve function `name` from instance `instance`
    /// with statically-typed parameter type `T`.
    #[instrument(level = "trace", skip(self))]
    fn serve_static<T>(
        &self,
        instance: &str,
        name: &str,
    ) -> impl Future<
        Output = anyhow::Result<
            Self::InvocationStream<Self::Context, T, <Self::Acceptor as Acceptor>::Transmitter>,
        >,
    > + Send
    where
        T: for<'a> Receive<'a> + Subscribe + 'static,
    {
        self.serve(
            instance,
            name,
            tower::service_fn(IncomingInvocation::accept_static),
        )
    }

    /// Serve function `name` from instance `instance` with dynamically-typed parameter type
    /// `params`.
    #[instrument(level = "trace", skip(self, params))]
    fn serve_dynamic(
        &self,
        instance: &str,
        name: &str,
        params: Arc<[Type]>,
    ) -> impl Future<
        Output = anyhow::Result<
            Self::InvocationStream<
                Self::Context,
                Vec<Value>,
                <Self::Acceptor as Acceptor>::Transmitter,
            >,
        >,
    > + Send {
        self.serve(
            instance,
            name,
            tower::service_fn(
                move |invocation: IncomingInvocation<
                    Self::Context,
                    Self::Subscriber,
                    Self::Acceptor,
                >| {
                    let params = Arc::clone(&params);
                    invocation.accept_dynamic(params)
                },
            ),
        )
    }

    /// Constructs a new invocation to be sent to peer
    fn new_invocation(
        &self,
    ) -> OutgoingInvocation<Self::Invocation, Self::Subscriber, Self::Subject>;

    /// Invokes function `name` from instance `instance` with parameters `params` and
    /// statically-typed results of type `T`
    #[instrument(level = "trace", skip(self, params))]
    fn invoke_static<T>(
        &self,
        instance: &str,
        name: &str,
        params: impl Encode,
    ) -> impl Future<Output = anyhow::Result<(T, Self::Transmission)>> + Send
    where
        for<'a> T: Receive<'a> + Subscribe + Send,
    {
        let OutgoingInvocation {
            invocation,
            subscriber,
            result_subject,
            error_subject,
        } = self.new_invocation();

        async {
            let (mut results_rx, results_nested, mut error_rx) = try_join!(
                async {
                    subscriber
                        .subscribe(result_subject.clone())
                        .await
                        .context("failed to subscribe for result values")
                },
                async {
                    T::subscribe(&subscriber, result_subject.clone())
                        .await
                        .context("failed to subscribe for asynchronous result values")
                },
                async {
                    subscriber
                        .subscribe(error_subject)
                        .await
                        .context("failed to subscribe for error value")
                },
            )?;
            let (tx, tx_fail) = invocation
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
                results = async {
                    let payload = results_rx
                        .try_next()
                        .await
                        .context("failed to receive initial result chunk")?
                        .context("unexpected end of result stream")?;
                    T::receive(payload, &mut results_rx, results_nested).await
                } => {
                    trace!("received results");
                    let (results, _) = results?;
                    Ok((results, tx))
                }
                payload = error_rx.try_next() => {
                    let payload = payload
                        .context("failed to receive initial error chunk")?
                        .context("unexpected end of error stream")?;
                    trace!("received error");
                    let (err, _) = String::receive_sync(payload, &mut error_rx)
                        .await
                        .context("failed to receive error string")?;
                    bail!(err)
                }
            }
        }
    }

    /// Invokes function `name` from instance `instance` with parameters `params` and
    /// dynamically-typed results of type `results`
    #[instrument(level = "trace", skip(self, params, results))]
    fn invoke_dynamic(
        &self,
        instance: &str,
        name: &str,
        params: impl Encode,
        results: &[Type],
    ) -> impl Future<Output = anyhow::Result<(Vec<Value>, Self::Transmission)>> + Send {
        let OutgoingInvocation {
            invocation,
            subscriber,
            result_subject,
            error_subject,
        } = self.new_invocation();

        async {
            let (mut results_rx, results_nested, mut error_rx) = try_join!(
                async {
                    subscriber
                        .subscribe(result_subject.clone())
                        .await
                        .context("failed to subscribe for result values")
                },
                async {
                    subscriber
                        .subscribe_tuple(result_subject.clone(), results)
                        .await
                        .context("failed to subscribe for asynchronous result values")
                },
                async {
                    subscriber
                        .subscribe(error_subject)
                        .await
                        .context("failed to subscribe for error value")
                },
            )?;
            let (tx, tx_fail) = invocation
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
                results = async {
                    let payload = results_rx
                        .try_next()
                        .await
                        .context("failed to receive initial result chunk")?
                        .context("unexpected end of result stream")?;
                    ReceiveContext::receive_tuple_context(
                        results,
                        payload,
                        &mut results_rx,
                        results_nested,
                    )
                    .await
                } => {
                    trace!("received results");
                    let (results, _) = results?;
                    Ok((results, tx))
                }
                payload = error_rx.next() => {
                    let payload = payload
                        .context("failed to receive initial error chunk")?
                        .context("unexpected end of error stream")?;
                    trace!("received error");
                    let (err, _) = String::receive_sync(payload, &mut error_rx)
                        .await
                        .context("failed to receive error string")?;
                    bail!(err)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;

    use super::*;

    #[allow(unused)]
    async fn encode_bounds(client: &impl Client) {
        async fn test(client: &impl Client, v: impl Encode) -> anyhow::Result<()> {
            let (res, tx) = client.invoke_static("instance", "name", "test").await?;
            tx.await?;
            res
        }

        struct X;

        #[async_trait]
        impl Encode for X {
            async fn encode(
                self,
                _: &mut (impl BufMut + Send),
            ) -> anyhow::Result<Option<AsyncValue>> {
                unreachable!()
            }
        }

        #[async_trait]
        impl Encode for &X {
            async fn encode(
                self,
                _: &mut (impl BufMut + Send),
            ) -> anyhow::Result<Option<AsyncValue>> {
                unreachable!()
            }
        }

        struct Y<'a> {
            pub x: &'a X,
        }

        #[async_trait]
        impl Encode for Y<'_> {
            async fn encode(
                self,
                _: &mut (impl BufMut + Send),
            ) -> anyhow::Result<Option<AsyncValue>> {
                unreachable!()
            }
        }

        #[async_trait]
        impl Encode for &Y<'_> {
            async fn encode(
                self,
                _: &mut (impl BufMut + Send),
            ) -> anyhow::Result<Option<AsyncValue>> {
                unreachable!()
            }
        }

        test(client, X);
        test(client, &X);
        test(client, &&X);

        test(client, [X].as_slice());
        test(client, &[X].as_slice());
        test(client, &&[X].as_slice());
        test(client, &&&[X].as_slice());

        test(client, [&X]);
        test(client, &[&X]);
        test(client, &&[&X]);
        test(client, &&&[&X]);

        test(client, [&X].as_slice());
        test(client, &[&X].as_slice());
        test(client, &&[&X].as_slice());
        test(client, &&&[&X].as_slice());

        test(client, vec![X]);
        test(client, &vec![X]);
        test(client, &&vec![X]);
        test(client, &&&vec![X]);

        test(client, vec![&X]);
        test(client, &vec![&X]);
        test(client, &&vec![&X]);
        test(client, &&&vec![&X]);

        test(client, [1, 2, 3]);
        test(client, &[1, 2, 3]);
        test(client, [1, 2, 3].as_slice());
        test(client, &[1, 2, 3].as_slice());

        test(client, b"test");
        test(client, &b"test");
        test(client, b"test".as_slice());
        test(client, &b"test".as_slice());

        async fn test_y(client: &impl Client, ys: &[Y<'_>]) -> anyhow::Result<()> {
            test(client, ys).await
        }
    }

    #[allow(unused)]
    async fn lifetimes(
        client: &impl super::Client,
        instance: &str,
    ) -> anyhow::Result<impl Stream<Item = ()>> {
        let name = String::from("");
        let invocations = client.serve_static::<()>(instance, &name).await?;
        Ok(invocations.map(|invocation| ()))
    }

    #[tokio::test]
    async fn codec() -> anyhow::Result<()> {
        tracing_subscriber::registry()
            .with(tracing_subscriber::fmt::layer().compact().without_time())
            .with(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
            )
            .init();

        let mut sub = None::<
            AsyncSubscriptionDemux<
                Box<dyn Stream<Item = anyhow::Result<Bytes>> + Send + Sync + Unpin>,
            >,
        >;

        let (items, payload) = super::Value::receive_stream_item_context(
            Some(&Type::U8),
            Bytes::from_static(&[1, 0x42, 2, 0xff, 0xfe]),
            &mut stream::empty(),
            &mut sub,
            0,
        )
        .await?;
        let items = items.unwrap();
        assert_eq!(items.len(), 1);
        let mut items = items.into_iter().map(|v| v.unwrap());

        let item = items.next().unwrap();
        let Value::U8(v) = item else {
            bail!("value type mismatch");
        };
        assert_eq!(v, 0x42);

        let (items, payload) = super::Value::receive_stream_item_context(
            Some(&Type::U8),
            payload,
            &mut stream::empty(),
            &mut sub,
            0,
        )
        .await?;
        let items = items.unwrap();
        assert_eq!(items.len(), 2);
        let mut items = items.into_iter().map(|v| v.unwrap());

        let item = items.next().unwrap();
        let Value::U8(v) = item else {
            bail!("value type mismatch");
        };
        assert_eq!(v, 0xff);

        let item = items.next().unwrap();
        let Value::U8(v) = item else {
            bail!("value type mismatch");
        };
        assert_eq!(v, 0xfe);
        assert!(!payload.has_remaining());
        Ok(())
    }
}
