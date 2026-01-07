//! Synchronous wrapper for value decoder using futures::executor
//!
//! This module provides a synchronous wrapper around the async
//! `read_value` function. It uses `futures::executor::block_on` which works
//! without a tokio runtime,
//!
//! # Design
//!
//! - Uses `std::io::Cursor` to wrap byte slices
//! - Blocks on async `read_value` using `futures::executor::block_on`
//! - Errors if there's unconsumed data (stricter than `wrpc-pack::unpack`)
//!
//! Note: `unpack` is not supported, as
//! 1. `wasm-wave` doesn't't expose (`pub(super)`) its internal type system,
//!     so it can't be used a generic passed to `unpack<T>`
//! 2. `unpack` only accepts one argument (`buf`)
//!    but, because of (1), we need to be able to pass a `Type` to decode

use std::io::Cursor;

use anyhow::{bail, Context as _};
use futures::executor::block_on;
use wasm_wave::value::{Type, Value};

use crate::core::decode::read_value;

/// Synchronously decode a wasm-wave value from bytes.
///
/// This is a synchronous wrapper around the async [`read_value`] function,
/// using `futures::executor::block_on`
///
/// # Errors
///
/// Returns an error if:
/// - The data is incomplete for the given type
/// - The data is malformed
/// - There are unconsumed bytes after decoding (stricter than `wrpc-pack::unpack`)
///
/// # Examples
///
/// ```
/// use wasm_wave::value::Type;
/// use wasm_wave::wasm::WasmValue;
/// use wrpc_wave::read_value_sync;
///
/// let data = vec![42]; // u8 value
/// let value = read_value_sync(&Type::U8, &data).unwrap();
/// assert_eq!(value.unwrap_u8(), 42);
/// ```
pub fn read_value_sync(ty: &Type, data: &[u8]) -> anyhow::Result<Value> {
    let mut cursor = Cursor::new(data);

    let value = block_on(async {
        let mut pinned = std::pin::pin!(&mut cursor);
        read_value(&mut pinned, ty).await
    })
    .context("failed to decode value")?;

    // Safety: Error on unconsumed data
    let consumed = cursor.position() as usize;
    if consumed < data.len() {
        bail!(
            "unconsumed data: {} of {} bytes remain",
            data.len() - consumed,
            data.len()
        );
    }

    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_wave::wasm::WasmValue as _;

    // Note: These tests focus on the sync wrapper's specific behavior (blocking,
    // unconsumed data detection). The underlying async decoder is thoroughly tested
    // in core/decode.rs - we don't duplicate all those test cases here.

    #[test]
    fn test_sync_wrapper_works() {
        // Verify the sync wrapper successfully decodes a simple value
        let data = vec![42];
        let value = read_value_sync(&Type::U8, &data).unwrap();
        assert_eq!(value.unwrap_u8(), 42);
    }

    #[test]
    fn test_unconsumed_data_error() {
        // The sync wrapper should error if there's unconsumed data after decoding
        // (This is stricter than wrpc-pack::unpack which ignores leftover bytes)
        let data = vec![42, 99, 100]; // u8 only needs 1 byte
        let result = read_value_sync(&Type::U8, &data);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("unconsumed data"));
        assert!(err_msg.contains("2 of 3 bytes remain"));
    }

    #[test]
    fn test_incomplete_data_error() {
        // The sync wrapper should propagate errors from the underlying async decoder
        let mut data = vec![3]; // length = 3 in leb128
        data.extend_from_slice(&[10, 20]); // only 2 elements instead of 3
        let ty = Type::list(Type::U8);
        let result = read_value_sync(&ty, &data);
        assert!(result.is_err(), "Should fail when data is incomplete");
    }
}
