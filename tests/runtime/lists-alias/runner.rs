use ::wit_bindgen_wrpc::bytes::Bytes;

use crate::runner::cat::*;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    foo(wrpc, (), &Bytes::from_static(b"hello")).await?;

    let t: Bytes = bar(wrpc, ()).await?;
    assert_eq!(t, &b"world"[..]);
    Ok(())
}
