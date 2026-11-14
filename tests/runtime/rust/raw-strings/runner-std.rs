//@ args = '--raw-strings'

use crate::client::cat::{bar, foo};

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Connect<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    foo(wrpc, (), b"hello").await?;

    let t: Vec<u8> = bar(wrpc, ()).await?;
    assert_eq!(t, b"world");
    Ok(())
}
