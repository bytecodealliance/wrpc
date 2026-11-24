//@ wasmtime-flags = '-Wcomponent-model-async'

use crate::client::a::b::i::*;

// Explicitly require Send.
fn require_send<T: Send>(t: T) -> T {
    t
}

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let clt = wrpc.connect().await?;
    require_send(async { one_argument(clt, "hello").await }).await?;
    Ok(())
}
