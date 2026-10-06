use bytes::BytesMut;
use tokio_util::codec::Encoder as _;
use wasm_wave::value::Type;
use wasm_wave::wasm::WasmValue as _;
use wasmtime::component::Val;
use wrpc_wave::{WaveEncoder, read_value_sync};

#[test]
fn wasmtime_compat() -> anyhow::Result<()> {
    let mut buf = BytesMut::new();
    WaveEncoder::new(&wasmtime::component::Type::U32).encode(&Val::U32(42), &mut buf)?;
    let v = read_value_sync(&Type::U32, &buf)?;
    assert_eq!(v.unwrap_u32(), 42);
    Ok(())
}
