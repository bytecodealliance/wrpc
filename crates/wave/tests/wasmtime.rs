use bytes::BytesMut;
use tokio_util::codec::Encoder as _;
use wasm_wave::value::{Type, Value};
use wasm_wave::wasm::WasmValue as _;
use wasmtime::component::Val;
use wrpc_wave::{ValueEncoder, decode_value};

#[test]
fn wasmtime_compat() -> anyhow::Result<()> {
    let mut buf = BytesMut::default();
    ValueEncoder::new(&wasmtime::component::Type::U32).encode(&Val::U32(42), &mut buf)?;
    let v = decode_value::<Value>(&buf, &Type::U32)?;
    assert_eq!(v.unwrap_u32(), 42);

    let mut buf = BytesMut::default();
    ValueEncoder::new(&Type::U32).encode(&Value::make_u32(42), &mut buf)?;
    let v = decode_value::<Val>(&buf, &wasmtime::component::Type::U32)?;
    assert_eq!(v, Val::U32(42));
    Ok(())
}
