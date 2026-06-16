use core::future::Future;
use core::ops::{Deref, DerefMut};

use futures::{Stream, StreamExt as _};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;

/// Accepts connections on a transport
pub trait Accept {
    /// Transport-specific invocation context
    type Context: Send + Sync + 'static;

    /// Outgoing byte stream
    type Outgoing: AsyncWrite + Send + Sync + Unpin + 'static;

    /// Incoming byte stream
    type Incoming: AsyncRead + Send + Sync + Unpin + 'static;

    /// Accept a connection returning a pair of streams and connection context
    fn accept(
        &mut self,
    ) -> impl Future<Output = std::io::Result<(Self::Context, Self::Outgoing, Self::Incoming)>>;
}

impl<T> Accept for &mut T
where
    T: Accept,
{
    type Context = T::Context;
    type Outgoing = T::Outgoing;
    type Incoming = T::Incoming;

    async fn accept(&mut self) -> std::io::Result<(Self::Context, Self::Outgoing, Self::Incoming)> {
        (**self).accept().await
    }
}

/// Wrapper returned by [`AcceptExt::map_context`]
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

/// Extension trait for [Accept]
pub trait AcceptExt: Accept + Sized {
    /// Maps [`Self::Context`](Accept::Context) to a type `T` using `F`
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

    async fn accept(&mut self) -> std::io::Result<(Self::Context, Self::Outgoing, Self::Incoming)> {
        let (cx, tx, rx) = self.inner.accept().await?;
        Ok(((self.f)(cx), tx, rx))
    }
}

impl<'a, T, U, F> Accept for &'a AcceptMapContext<T, F>
where
    &'a T: Accept,
    U: Send + Sync + 'static,
    F: Fn(<&'a T as Accept>::Context) -> U,
{
    type Context = U;
    type Outgoing = <&'a T as Accept>::Outgoing;
    type Incoming = <&'a T as Accept>::Incoming;

    async fn accept(&mut self) -> std::io::Result<(Self::Context, Self::Outgoing, Self::Incoming)> {
        let (cx, tx, rx) = <&'a T>::accept(&mut &self.inner).await?;
        Ok(((self.f)(cx), tx, rx))
    }
}

/// A wrapper around a [Stream] of connections
pub struct AcceptStream<T>(T);

impl<T> From<T> for AcceptStream<T> {
    fn from(stream: T) -> Self {
        Self(stream)
    }
}

impl<T, C, O, I> Accept for AcceptStream<T>
where
    T: Stream<Item = (C, O, I)> + Unpin,
    C: Send + Sync + 'static,
    O: AsyncWrite + Send + Sync + Unpin + 'static,
    I: AsyncRead + Send + Sync + Unpin + 'static,
{
    type Context = C;
    type Outgoing = O;
    type Incoming = I;

    async fn accept(&mut self) -> std::io::Result<(Self::Context, Self::Outgoing, Self::Incoming)> {
        let Some((cx, tx, rx)) = self.0.next().await else {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        };
        Ok((cx, tx, rx))
    }
}

/// A wrapper around an [mpsc::Receiver] of connections
pub struct AcceptReceiver<C, O, I>(mpsc::Receiver<(C, O, I)>);

impl<C, O, I> From<mpsc::Receiver<(C, O, I)>> for AcceptReceiver<C, O, I> {
    fn from(stream: mpsc::Receiver<(C, O, I)>) -> Self {
        Self(stream)
    }
}

impl<C, O, I> Accept for AcceptReceiver<C, O, I>
where
    C: Send + Sync + 'static,
    O: AsyncWrite + Send + Sync + Unpin + 'static,
    I: AsyncRead + Send + Sync + Unpin + 'static,
{
    type Context = C;
    type Outgoing = O;
    type Incoming = I;

    async fn accept(&mut self) -> std::io::Result<(Self::Context, Self::Outgoing, Self::Incoming)> {
        let Some((cx, tx, rx)) = self.0.recv().await else {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        };
        Ok((cx, tx, rx))
    }
}

/// A wrapper around an [mpsc::UnboundedReceiver] of connections
pub struct AcceptUnboundedReceiver<C, O, I>(mpsc::UnboundedReceiver<(C, O, I)>);

impl<C, O, I> From<mpsc::UnboundedReceiver<(C, O, I)>> for AcceptUnboundedReceiver<C, O, I> {
    fn from(stream: mpsc::UnboundedReceiver<(C, O, I)>) -> Self {
        Self(stream)
    }
}

impl<C, O, I> Accept for AcceptUnboundedReceiver<C, O, I>
where
    C: Send + Sync + 'static,
    O: AsyncWrite + Send + Sync + Unpin + 'static,
    I: AsyncRead + Send + Sync + Unpin + 'static,
{
    type Context = C;
    type Outgoing = O;
    type Incoming = I;

    async fn accept(&mut self) -> std::io::Result<(Self::Context, Self::Outgoing, Self::Incoming)> {
        let Some((cx, tx, rx)) = self.0.recv().await else {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        };
        Ok((cx, tx, rx))
    }
}
