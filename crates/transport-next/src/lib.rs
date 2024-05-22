use core::future::Future;
use core::str::FromStr;

use futures::Stream;
use tokio::io::{AsyncRead, AsyncWrite};

pub trait Index<T> {
    type Error;

    fn index(&self, path: &[usize]) -> Result<T, Self::Error>;
}

pub trait Session {
    type Error: FromStr + ToString;
    type TransportError;

    fn finish(
        self,
        res: Result<(), Self::Error>,
    ) -> impl Future<Output = Result<Result<(), Self::Error>, Self::TransportError>>;
}

pub struct Invocation<O, I, S> {
    pub outgoing: O,
    pub incoming: I,
    pub session: S,
}

pub trait Invoke {
    type Error;
    type Context;
    type Session: Session;
    type Outgoing: AsyncWrite + Index<Self::NestedOutgoing>;
    type NestedOutgoing: AsyncWrite + Index<Self::NestedOutgoing>;
    type Incoming: AsyncRead + Index<Self::Incoming>;

    /// Invoke function `func` on instance `instance`
    fn invoke(
        &self,
        cx: Self::Context,
        instance: &str,
        func: &str,
        paths: &[&[Option<usize>]],
    ) -> impl Future<
        Output = Result<Invocation<Self::Outgoing, Self::Incoming, Self::Session>, Self::Error>,
    >;
}

pub trait Serve {
    type Error;
    type Context;
    type Session: Session;
    type Outgoing: AsyncWrite + Index<Self::Outgoing>;
    type Incoming: AsyncRead + Index<Self::Incoming>;

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
    >;
}
