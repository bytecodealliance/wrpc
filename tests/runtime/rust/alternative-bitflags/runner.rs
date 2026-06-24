//@ args = '--bitflags-path ::wit_bindgen_wrpc::bitflags'

use crate::runner::my::inline::t::{get_flag, Bar};

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    assert_eq!(get_flag(wrpc, ()).await?, Bar::BAZ);
    Ok(())
}
