use core::borrow::BorrowMut;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{FramedRead, FramedWrite};
use wasmtime::component::{Accessor, Func, Val};
use wasmtime::error::Context as _;
use wrpc_transport::{Codec, FrameDecoder, FrameEncoder};

use crate::{TupleEncoder, ValDecoder, WrpcView};

/// Calls the exported `func` with parameters received from the frames `rx` and transmits the
/// results to the frames `tx` using `codec`, which must match the type of `func`.
///
/// The `store` [`Accessor`] is used to call `func` concurrently, it can be acquired, for example,
/// using [`wasmtime::Store::run_concurrent`].
pub async fn call<T, I, O>(
    store: &Accessor<T>,
    codec: Codec<TupleEncoder<'_, &Accessor<T>>, ValDecoder<'_, &Accessor<T>>>,
    tx: impl BorrowMut<FramedWrite<O, FrameEncoder>>,
    rx: impl BorrowMut<FramedRead<I, FrameDecoder>>,
    func: Func,
) -> wasmtime::Result<()>
where
    T: WrpcView + 'static,
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
{
    let n = codec.enc.tys.len();
    codec
        .serve(tx, rx, async |params: Vec<Val>| {
            let mut results = vec![Val::Bool(false); n];
            func.call_concurrent(store, &params, &mut results)
                .await
                .context("failed to call function")?;
            Ok(results)
        })
        .await
}
