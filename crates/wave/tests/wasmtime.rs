use bytes::BytesMut;
use tokio_util::codec::{Decoder as _, Encoder as _};
use wasm_wave::value::{Type, Value};
use wasm_wave::wasm::WasmValue as _;
use wasmtime::component::Val;
use wrpc_wave::{ValueDecoder, ValueEncoder};

#[test]
fn wasmtime_compat() -> anyhow::Result<()> {
    let mut buf = BytesMut::default();
    ValueEncoder::new(wasmtime::component::Type::U32).encode(&Val::U32(42), &mut buf)?;
    let v = ValueDecoder::<Value>::new(Type::U32).decode(&mut buf)?;
    assert_eq!(v, Some(Value::make_u32(42)));

    ValueEncoder::new(Type::U32).encode(&Value::make_u32(42), &mut buf)?;
    let v = ValueDecoder::<Val>::new(wasmtime::component::Type::U32).decode(&mut buf)?;
    assert_eq!(v, Some(Val::U32(42)));
    Ok(())
}
