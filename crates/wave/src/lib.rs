//! wRPC encoding and decoding of values implementing [`wasm_wave`] traits
//!
//! Values are encoded with [`ValueEncoder`], or with [`wrpc_transport::Encode`] when wrapped
//! in a [`TypedValue`], and decoded with [`ValueDecoder`]. Function parameters and results are
//! encoded with [`TupleEncoder`] and decoded with [`TupleDecoder`].
//!
//! With [`wasm_wave::value::Type`], handles are represented by `list<u8>`, futures by
//! [`future_type`] and streams by [`stream_type`]. Only `ready` futures and streams are
//! supported: encoding a `pending` one fails with [`std::io::ErrorKind::WouldBlock`] and decoding
//! one with [`std::io::ErrorKind::InvalidData`], like [`wrpc_transport::pack`] and
//! [`wrpc_transport::unpack`].
//!
//! ```
//! use bytes::BytesMut;
//! use tokio_util::codec::{Decoder as _, Encoder as _};
//! use wasm_wave::value::{Type, Value};
//! use wasm_wave::wasm::WasmValue as _;
//! use wrpc_wave::{ValueDecoder, ValueEncoder};
//!
//! let ty = Type::list(Type::U32);
//! let v = Value::make_list(&ty, [Value::make_u32(1), Value::make_u32(2)]).unwrap();
//! let mut buf = BytesMut::default();
//! ValueEncoder::new(ty.clone()).encode(&v, &mut buf).unwrap();
//! assert_eq!(ValueDecoder::new(ty).decode(&mut buf).unwrap(), Some(v));
//! ```

mod decode;
mod encode;
mod typed;

use wasm_wave::value::Type;
use wasm_wave::wasm::WasmType;

pub use decode::{DEFAULT_MAX_LEN, TupleDecoder, ValueDecoder};
pub use encode::{TupleEncoder, ValueEncoder};
pub use typed::{TypedValue, TypedValueEncoder};

const PENDING: &str = "<pending>";

/// Returns the type of `future<T>` values, represented as `ready(T)`, or `ready` without `T`
#[must_use]
pub fn future_type(ty: Option<Type>) -> Type {
    Type::variant([(PENDING, None), ("ready", ty)]).expect("variant has cases")
}

/// Returns the type of `stream<T>` values, represented as `ready(list<T>)`, or `ready(u32)`
/// carrying the number of elements without `T`
#[must_use]
pub fn stream_type(ty: Option<Type>) -> Type {
    let ty = ty.map_or(Type::U32, Type::list);
    future_type(Some(ty))
}

fn is_pending(ty: &impl WasmType, case: usize) -> bool {
    case == 0
        && ty
            .variant_cases()
            .next()
            .is_some_and(|(name, ..)| name == PENDING)
}
