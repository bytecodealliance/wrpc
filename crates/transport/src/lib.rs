use core::fmt::Display;
use core::future::Future;
use core::str::FromStr;

use bytes::Bytes;
use futures::Stream;
use tokio::io::{AsyncRead, AsyncWrite};

/// `Index` implementations are capable of multiplexing underlying connections using a particular
/// structural `path`
pub trait Index<T> {
    /// Multiplexing error
    type Error: Sync + Send;

    /// Index the entity using a structural `path`
    fn index(&self, path: &[usize]) -> Result<T, Self::Error>;
}

/// Invocation session, which is used to track the lifetime of the data exchange and for
/// error reporting.
pub trait Session {
    type Error: FromStr + Display + Sync + Send;
    type TransportError: Sync + Send;

    /// Finish the invocation session, closing all associated communication channels and reporting
    /// a result to the peer.
    fn finish(
        self,
        res: Result<(), Self::Error>,
    ) -> impl Future<Output = Result<Result<(), Self::Error>, Self::TransportError>> + Send;
}

/// Invocation encapsulates either server or client-side triple of:
/// - Multiplexed outgoing byte stream
/// - Multiplexed incoming byte stream
/// - Active session, used to communicate and receive transport-layer errors
pub struct Invocation<O, I, S> {
    /// Outgoing multiplexed byte stream
    pub outgoing: O,

    /// Incoming multiplexed byte stream
    pub incoming: I,

    /// Invocation session
    pub session: S,
}

/// Client-side handle to a wRPC transport
pub trait Invoke: Sync + Send {
    /// Transport-specific error
    type Error: Sync + Send;

    /// Transport-specific invocation context
    type Context: Sync + Send;

    /// Transport-specific session used for lifetime tracking and error reporting
    type Session: Session + Sync + Send;

    /// Outgoing multiplexed byte stream
    type Outgoing: AsyncWrite + Index<Self::NestedOutgoing> + Sync + Send;

    /// Outgoing multiplexed byte stream, nested at a particular path
    type NestedOutgoing: AsyncWrite + Index<Self::NestedOutgoing> + Sync + Send;

    /// Incoming multiplexed byte stream
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

/// Server-side handle to a wRPC transport
pub trait Serve {
    /// Transport-specific error
    type Error: Sync + Send;

    /// Transport-specific invocation context
    type Context: Sync + Send;

    /// Transport-specific session used for lifetime tracking and error reporting
    type Session: Session + Sync + Send;

    /// Outgoing multiplexed byte stream
    type Outgoing: AsyncWrite + Index<Self::Outgoing> + Sync + Send;

    /// Incoming multiplexed byte stream
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
