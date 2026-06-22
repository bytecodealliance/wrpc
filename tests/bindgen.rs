//! Drives the `wit-bindgen-wrpc test` subcommand over the in-tree codegen and
//! runtime test suites. Runs as an ordinary integration test so it is exercised
//! by `cargo test`/`nextest`, locating the freshly-built CLI via the
//! `CARGO_BIN_EXE_wit-bindgen-wrpc` environment variable Cargo provides.
#![cfg(feature = "bin-bindgen")]

use std::process::Command;

#[test]
fn bindgen() {
    let status = Command::new(env!("CARGO_BIN_EXE_wit-bindgen-wrpc"))
        .args([
            "test",
            "tests/codegen",
            "tests/runtime",
            "--artifacts",
            concat!(env!("CARGO_TARGET_TMPDIR"), "/bindgen"),
            "--rust-wit-bindgen-path",
            "crates/wit-bindgen",
            "--allow-fail-list",
            "tests/codegen.skip",
        ])
        .status()
        .expect("failed to run `wit-bindgen-wrpc test`");
    assert!(
        status.success(),
        "`wit-bindgen-wrpc test` reported failures"
    );
}
