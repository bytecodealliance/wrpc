//! wRPC encoding and decoding of values implementing [`wasm_wave`] traits
//!
//! Values are encoded with [`ValueEncoder`], or with [`wrpc_transport::Encode`] when wrapped
//! in a [`TypedValue`], and decoded with [`ValueDecoder`]. Function parameters and results are
//! encoded with [`TupleEncoder`] and decoded with [`TupleDecoder`].
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

pub use decode::{DEFAULT_MAX_LEN, TupleDecoder, ValueDecoder};
pub use encode::{TupleEncoder, ValueEncoder};
pub use typed::{TypedValue, TypedValueEncoder};
