#![allow(clippy::type_complexity)] // TODO: https://github.com/bytecodealliance/wrpc/issues/2

//! wRPC integration for Wasmtime
//!
//! [`TupleDecoder`] decodes the parameters of an invocation into store-free [`Val`]s and
//! [`TupleEncoder`] encodes its results. Values without resources, `future`s and `stream`s convert
//! from and into [`wasmtime::component::Val`] using [`TryFrom`], the others are moved into a
//! store by [`lift`] and out of it by [`lower`].
//!
//! [`Exports`] looks up the component function export by the instance and function names of the
//! invocation. The host owns the I/O, stores, instances and resource handles and calls the
//! function, for example, it instantiates the component for each invocation or serves all
//! connections in one instance from a single `run_concurrent` event loop, which guest-exported
//! resources require.

macro_rules! with_payload {
    ($ty:expr, $p:ident => $body:expr, $unsupported:expr) => {
        match $ty {
            None => {
                type $p = ();
                $body
            }
            Some(ref ty) => with_payload!(@some ty, $p => $body, $unsupported),
        }
    };
    (@some $ty:expr, $p:ident => $body:expr, $unsupported:expr) => {
        match $ty {
            wasmtime::component::Type::Bool => {
                type $p = bool;
                $body
            }
            wasmtime::component::Type::S8 => {
                type $p = i8;
                $body
            }
            wasmtime::component::Type::U8 => {
                type $p = u8;
                $body
            }
            wasmtime::component::Type::S16 => {
                type $p = i16;
                $body
            }
            wasmtime::component::Type::U16 => {
                type $p = u16;
                $body
            }
            wasmtime::component::Type::S32 => {
                type $p = i32;
                $body
            }
            wasmtime::component::Type::U32 => {
                type $p = u32;
                $body
            }
            wasmtime::component::Type::S64 => {
                type $p = i64;
                $body
            }
            wasmtime::component::Type::U64 => {
                type $p = u64;
                $body
            }
            wasmtime::component::Type::Float32 => {
                type $p = f32;
                $body
            }
            wasmtime::component::Type::Float64 => {
                type $p = f64;
                $body
            }
            wasmtime::component::Type::Char => {
                type $p = char;
                $body
            }
            wasmtime::component::Type::String => {
                type $p = String;
                $body
            }
            _ => $unsupported,
        }
    };
}

mod codec;
mod export;
mod val;

pub use codec::*;
pub use export::*;
pub use val::*;
