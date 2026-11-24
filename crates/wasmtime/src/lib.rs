#![allow(clippy::type_complexity)] // TODO: https://github.com/bytecodealliance/wrpc/issues/2

//! wRPC integration for Wasmtime
//!
//! [`TupleDecoder`] decodes the parameters of an invocation into store-free [`Val`]s and
//! [`TupleEncoder`] encodes its results, [`ValDecoder`] and [`ValEncoder`] do the same for a
//! single value. Values without resources, `future`s and `stream`s convert from and into
//! [`wasmtime::component::Val`] using [`TryFrom`], the others are moved into a store by [`lift`]
//! and out of it by [`lower`]. The encoders also encode borrowed [`wasmtime::component::Val`]s
//! without resources, `future`s and `stream`s.
//!
//! [`Exports`] looks up the component function export by the instance and function names of the
//! invocation. The host owns the I/O, stores, instances and resource handles and calls the
//! function, for example, it instantiates the component for each invocation or serves all
//! connections in one instance from a single `run_concurrent` event loop, which guest-exported
//! resources require.
//!
//! Received `future`s and `stream`s are not buffered, so the guest must read the ones of an
//! invocation concurrently: a guest reading one `stream` to its end before another one stalls the
//! invocation indefinitely, if the peer interleaves them (see [`wrpc_core`]).

mod codec;
mod export;
mod val;

pub use codec::*;
pub use export::*;
pub use val::*;
