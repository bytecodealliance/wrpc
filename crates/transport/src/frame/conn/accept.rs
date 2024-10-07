use core::future::Future;
use core::ops::{Deref, DerefMut};

use tokio::io::{AsyncRead, AsyncWrite};

pub trait Accept {
    /// Transport-specific invocation context
    type Context: Send + Sync + 'static;

    /// Outgoing byte stream
    type Outgoing: AsyncWrite + Send + Sync + Unpin + 'static;

    /// Incoming byte stream
    type Incoming: AsyncRead + Send + Sync + Unpin + 'static;

    fn accept(
        &self,
    ) -> impl Future<Output = std::io::Result<(Self::Context, Self::Outgoing, Self::Incoming)>>;
}

pub struct AcceptMapContext<T, F> {
    inner: T,
    f: F,
}

impl<T, F> Deref for AcceptMapContext<T, F> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T, F> DerefMut for AcceptMapContext<T, F> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

pub trait AcceptExt: Accept + Sized {
    fn map_context<T, F: Fn(Self::Context) -> T>(self, f: F) -> AcceptMapContext<Self, F> {
        AcceptMapContext { inner: self, f }
    }
}

impl<T: Accept> AcceptExt for T {}

impl<T, U, F> Accept for AcceptMapContext<T, F>
where
    T: Accept,
    U: Send + Sync + 'static,
    F: Fn(T::Context) -> U,
{
    type Context = U;
    type Outgoing = T::Outgoing;
    type Incoming = T::Incoming;

    async fn accept(&self) -> std::io::Result<(Self::Context, Self::Outgoing, Self::Incoming)> {
        (&self).accept().await
    }
}

impl<T, U, F> Accept for &AcceptMapContext<T, F>
where
    T: Accept,
    U: Send + Sync + 'static,
    F: Fn(T::Context) -> U,
{
    type Context = U;
    type Outgoing = T::Outgoing;
    type Incoming = T::Incoming;

    async fn accept(&self) -> std::io::Result<(Self::Context, Self::Outgoing, Self::Incoming)> {
        let (cx, tx, rx) = self.inner.accept().await?;
        Ok(((self.f)(cx), tx, rx))
    }
}
