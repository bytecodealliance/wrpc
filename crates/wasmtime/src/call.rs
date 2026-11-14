use core::borrow::BorrowMut;
use core::fmt;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::try_join;
use tokio_util::codec::FramedRead;
use tracing::debug;
use wasmtime::component::{Accessor, Func, Val};
use wasmtime::error::Context as _;
use wrpc_transport::{FrameDecoder, Server};

use crate::{TupleDecoder, TupleEncoder, WrpcView};

/// Error type returned by [call]
pub enum CallError {
    Decode(wasmtime::Error),
    Encode(wasmtime::Error),
    Call(wasmtime::Error),
    Write(wasmtime::Error),
    Closed(wasmtime::Error),
}

impl core::error::Error for CallError {}

impl fmt::Debug for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CallError::Decode(error)
            | CallError::Encode(error)
            | CallError::Call(error)
            | CallError::Write(error)
            | CallError::Closed(error) => error.fmt(f),
        }
    }
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CallError::Decode(error)
            | CallError::Encode(error)
            | CallError::Call(error)
            | CallError::Write(error)
            | CallError::Closed(error) => error.fmt(f),
        }
    }
}

/// Calls the exported `func` with parameters received from the frames `rx` and transmits the
/// results on `tx` using `srv`, the codecs of which must match the type of `func`.
///
/// The `store` [`Accessor`] is used to call `func` concurrently, it can be acquired, for example,
/// using [`wasmtime::Store::run_concurrent`].
pub async fn call<T, R, I, O>(
    store: &Accessor<T>,
    srv: Server<TupleEncoder<'_, &Accessor<T>>, TupleDecoder<'_, &Accessor<T>>>,
    tx: O,
    rx: R,
    func: Func,
) -> Result<(), CallError>
where
    T: WrpcView + 'static,
    R: BorrowMut<FramedRead<I, FrameDecoder>>,
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
{
    let n = srv.enc.tys.len();
    let (params, outgoing) = srv.serve(tx, rx);
    let (params, rx) = params.await.map_err(|err| CallError::Decode(err.into()))?;
    try_join!(
        async { rx.await.map_err(|err| CallError::Closed(err.into())) },
        async {
            let mut results = vec![Val::Bool(false); n];
            func.call_concurrent(store, &params, &mut results)
                .await
                .context("failed to call function")
                .map_err(CallError::Call)?;
            let tx = outgoing
                .send(results.as_slice())
                .context("failed to encode results")
                .map_err(CallError::Encode)?;
            debug!("transmitting results");
            tx.await
                .context("failed to transmit results")
                .map_err(CallError::Write)
        },
    )?;
    Ok(())
}
