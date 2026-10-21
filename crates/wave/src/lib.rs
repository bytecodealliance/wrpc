//! wRPC encoding and decoding of values implementing [`wasm_wave`] traits
//!
//! Values are encoded with [`ValueEncoder`], or with [`wrpc_transport::Encode`] when wrapped
//! in a [`TypedValue`], and decoded with [`read_value`] or [`decode_value`].
//!
//! ```
//! use bytes::BytesMut;
//! use tokio_util::codec::Encoder as _;
//! use wasm_wave::value::{Type, Value};
//! use wasm_wave::wasm::WasmValue as _;
//! use wrpc_wave::{ValueEncoder, decode_value};
//!
//! let ty = Type::list(Type::U32);
//! let v = Value::make_list(&ty, [Value::make_u32(1), Value::make_u32(2)]).unwrap();
//! let mut buf = BytesMut::default();
//! ValueEncoder::new(&ty).encode(&v, &mut buf).unwrap();
//! assert_eq!(decode_value::<Value>(&buf, &ty).unwrap(), v);
//! ```

mod decode;
mod encode;
mod typed;

pub use decode::{decode_value, read_value};
pub use encode::ValueEncoder;
pub use typed::{TypedValue, TypedValueEncoder};
