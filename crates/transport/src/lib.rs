use core::fmt::Display;
use core::future::Future;
use core::str::FromStr;

use bytes::Bytes;
use futures::Stream;
use tokio::io::{AsyncRead, AsyncWrite};

pub trait Index<T> {
    type Error: Sync + Send;

    fn index(&self, path: &[usize]) -> Result<T, Self::Error>;
}

pub trait Session {
    type Error: FromStr + Display + Sync + Send;
    type TransportError: Sync + Send;

    fn finish(
        self,
        res: Result<(), Self::Error>,
    ) -> impl Future<Output = Result<Result<(), Self::Error>, Self::TransportError>> + Send;
}

pub struct Invocation<O, I, S> {
    pub outgoing: O,
    pub incoming: I,
    pub session: S,
}

pub trait Invoke: Sync + Send {
    type Error: Sync + Send;
    type Context: Sync + Send;
    type Session: Session + Sync + Send;
    type Outgoing: AsyncWrite + Index<Self::NestedOutgoing> + Sync + Send;
    type NestedOutgoing: AsyncWrite + Index<Self::NestedOutgoing> + Sync + Send;
    type Incoming: AsyncRead + Index<Self::Incoming> + Sync + Send;

    /// Invoke function `func` on instance `instance`
    fn invoke(
        &self,
        cx: Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: &[&[Option<usize>]],
    ) -> impl Future<
        Output = Result<Invocation<Self::Outgoing, Self::Incoming, Self::Session>, Self::Error>,
    > + Send;
}

pub trait Serve {
    type Error: Sync + Send;
    type Context: Sync + Send;
    type Session: Session + Sync + Send;
    type Outgoing: AsyncWrite + Index<Self::Outgoing> + Sync + Send;
    type Incoming: AsyncRead + Index<Self::Incoming> + Sync + Send;

    /// Serve function `func` from instance `instance`
    fn serve(
        &self,
        instance: &str,
        func: &str,
        paths: &[&[Option<usize>]],
    ) -> impl Future<
        Output = Result<
            impl Stream<
                Item = Result<
                    (
                        Self::Context,
                        Invocation<Self::Outgoing, Self::Incoming, Self::Session>,
                    ),
                    Self::Error,
                >,
            >,
            Self::Error,
        >,
    > + Send;
}
