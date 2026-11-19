use core::borrow::BorrowMut;

use std::sync::Arc;

use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{FramedRead, FramedWrite};
use wasmtime::component::{self, Accessor, Func, ResourceAny, Type};
use wasmtime::error::Context as _;
use wrpc_transport::{Codec, FrameDecoder, FrameEncoder};

use crate::{TupleDecoder, TupleEncoder, Val, lift, lower};

/// Calls the exported `func` with parameters received from the frames `rx` and transmits the
/// results to the frames `tx` using `codec`, which must match the type of `func`.
///
/// `lift_resource` and `lower_resource` map resource handles as in [`lift`] and [`lower`].
///
/// The `store` [`Accessor`] is used to call `func` concurrently, it can be acquired, for example,
/// using [`wasmtime::Store::run_concurrent`].
pub async fn call<T, I, O>(
    store: &Accessor<T>,
    codec: Codec<TupleEncoder, TupleDecoder>,
    tx: impl BorrowMut<FramedWrite<O, FrameEncoder>>,
    rx: impl BorrowMut<FramedRead<I, FrameDecoder>>,
    func: Func,
    lift_resource: impl FnMut(&mut T, &Type, Bytes) -> wasmtime::Result<ResourceAny>,
    lower_resource: impl FnMut(&mut T, &Type, ResourceAny) -> wasmtime::Result<Bytes>,
) -> wasmtime::Result<()>
where
    T: Send + 'static,
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
{
    let params_tys = Arc::clone(&codec.dec.tys);
    let results_tys = Arc::clone(&codec.enc.tys);
    codec
        .serve(tx, rx, async |params: Vec<Val>| {
            let params =
                store.with(|store| lift(store, params_tys.iter(), params, lift_resource))?;
            let mut results = vec![component::Val::Bool(false); results_tys.len()];
            func.call_concurrent(store, &params, &mut results)
                .await
                .context("failed to call function")?;
            store.with(|store| lower(store, results_tys.iter(), results, lower_resource))
        })
        .await
}
