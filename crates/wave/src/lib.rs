//! wRPC encoding/decoding support for wasm-wave values
//!
//! This crate provides encoding and decoding for wasm-wave values with wRPC transport.
//!
//! ## Encoding
//!
//! ### 1. Encoder approach (tokio-util codec)
//!
//! This approach mirrors the `ValEncoder` from `wrpc-runtime-wasmtime`,
//! where you create an encoder with a type reference and then encode values directly:
//!
//! ```no_run
//! use wrpc_wave::WaveEncoder;
//! use wasm_wave::value::{Value, Type};
//! use wasm_wave::wasm::WasmValue;
//! use bytes::BytesMut;
//! use wit_bindgen_wrpc::tokio_util::codec::Encoder;
//!
//! let value = Value::make_u32(42);
//! let ty = Type::U32;
//! let mut encoder = WaveEncoder::new(&ty);
//! let mut buf = BytesMut::new();
//! encoder.encode(&value, &mut buf).unwrap();
//! ```
//!
//! ### 2. `wrpc-pack` compatibility layer
//!
//! This approach bundles wasm-wave values with their types and uses the `wrpc_pack::pack` function:
//!
//! ```
//! # #[cfg(feature = "pack")]
//! # {
//! use wrpc_wave::WasmTypedValue;
//! use wasm_wave::value::{Value, Type};
//! use wasm_wave::wasm::WasmValue;
//! use wrpc_pack::pack;
//! use bytes::BytesMut;
//!
//! let value = Value::make_u32(42);
//! let typed_value = WasmTypedValue(value, Type::U32);
//! let mut buf = BytesMut::new();
//! pack(typed_value, &mut buf).unwrap();
//! # }
//! ```
//!
//! This approach requires the `pack` feature to be enabled and the `wrpc-pack` crate.
//!
//! ## Decoding
//!
//! ### 1. Async API (recommended)
//!
//! Use [`read_value`] for async decoding from any `AsyncRead` source:
//!
//! ```no_run
//! use wrpc_wave::read_value;
//! use wasm_wave::value::Type;
//!
//! # async fn example() -> std::io::Result<()> {
//! let mut stream = tokio::net::TcpStream::connect("127.0.0.1:8080").await?;
//! let mut pinned = std::pin::pin!(&mut stream);
//! let value = read_value(&mut pinned, &Type::U32).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ### 2. Sync API
//!
//! Use [`read_value_sync`] for synchronous decoding from byte slices
//! (requires the `sync` feature):
//!
//! ```
//! # #[cfg(feature = "sync")]
//! # {
//! use wrpc_wave::read_value_sync;
//! use wasm_wave::value::Type;
//! use wasm_wave::wasm::WasmValue;
//!
//! let data = vec![42]; // u8 value
//! let value = read_value_sync(&Type::U8, &data).unwrap();
//! assert_eq!(value.unwrap_u8(), 42);
//! # }
//! ```

// Core encoding/decoding - no feature flags required
mod core;

// Sync decoding - requires sync feature (uses futures::executor)
#[cfg(feature = "sync")]
mod sync;

// wrpc-pack integration - requires pack feature
#[cfg(feature = "pack")]
mod pack;

// Re-export async decoder
pub use core::decode::read_value;

// Re-export encoder
pub use core::encode::WaveEncoder;

// Re-export sync decoder (requires sync feature)
#[cfg(feature = "sync")]
pub use sync::decode::read_value_sync;

// Re-export wrpc-pack integration (requires pack feature)
#[cfg(feature = "pack")]
pub use pack::{TypedWaveEncoder, WasmTypedValue};
