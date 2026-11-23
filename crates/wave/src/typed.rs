//! [`wrpc_core::Encode`] support

use bytes::BytesMut;
use tokio_util::codec::Encoder;
use wasm_wave::value::Value;
use wasm_wave::wasm::WasmValue;
use wrpc_core::{Encode, SyncCodec};

use crate::encode::encode_value;

/// Value paired with its type, which implements [`Encode`]
///
/// ```
/// use bytes::BytesMut;
/// use wasm_wave::value::{Type, Value};
/// use wasm_wave::wasm::WasmValue as _;
/// use wrpc_core::SyncCodec;
/// use wrpc_wave::{TypedValue, TypedValueEncoder};
///
/// let mut buf = BytesMut::default();
/// let v = TypedValue(Value::make_u32(42), Type::U32);
/// wrpc_core::pack(SyncCodec(TypedValueEncoder), v, &mut buf).unwrap();
/// assert_eq!(buf.as_ref(), [42]);
/// ```
#[derive(Clone, Debug)]
pub struct TypedValue<V: WasmValue = Value>(pub V, pub V::Type);

/// [Encoder] of [`TypedValue`]
#[derive(Debug, Default)]
pub struct TypedValueEncoder;

impl<V: WasmValue> Encoder<&TypedValue<V>> for TypedValueEncoder {
    type Error = std::io::Error;

    fn encode(&mut self, v: &TypedValue<V>, dst: &mut BytesMut) -> std::io::Result<()> {
        encode_value(&v.1, &v.0, dst)
    }
}

impl<V: WasmValue> Encoder<TypedValue<V>> for TypedValueEncoder {
    type Error = std::io::Error;

    fn encode(&mut self, v: TypedValue<V>, dst: &mut BytesMut) -> std::io::Result<()> {
        self.encode(&v, dst)
    }
}

impl<V: WasmValue> Encode for TypedValue<V> {
    type Encoder = SyncCodec<TypedValueEncoder>;
}

impl<V: WasmValue> Encode for &TypedValue<V> {
    type Encoder = SyncCodec<TypedValueEncoder>;
}
