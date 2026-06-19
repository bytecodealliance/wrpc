#![cfg(target_family = "wasm")]

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
use web_sys::MessageChannel;
use wrpc_message_channel::connect;

wasm_bindgen_test_configure!(run_in_browser);

/// Exercises the [`connect`] byte-stream bridge over both ends of a
/// [`MessageChannel`]: bytes written to one port's outgoing half are read back
/// on the other port's incoming half, in both directions, terminated by the
/// end-of-stream sentinel.
#[wasm_bindgen_test]
async fn loopback() {
    let chan = MessageChannel::new().expect("failed to create `MessageChannel`");
    let (mut a_tx, mut a_rx) = connect(chan.port1());
    let (mut b_tx, mut b_rx) = connect(chan.port2());

    a_tx.write_all(b"ping")
        .await
        .expect("failed to write `ping`");
    a_tx.shutdown().await.expect("failed to shut down port 1");
    let mut buf = Vec::new();
    b_rx.read_to_end(&mut buf)
        .await
        .expect("failed to read `ping`");
    assert_eq!(buf, b"ping");

    b_tx.write_all(b"pong")
        .await
        .expect("failed to write `pong`");
    b_tx.shutdown().await.expect("failed to shut down port 2");
    let mut buf = Vec::new();
    a_rx.read_to_end(&mut buf)
        .await
        .expect("failed to read `pong`");
    assert_eq!(buf, b"pong");
}
