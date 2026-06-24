//@ args = '--bitflags-path crate::client::my_bitflags'

pub(crate) use ::wit_bindgen_wrpc::bitflags as my_bitflags;

use crate::client::my::inline::t::{get_flag, Bar};

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    assert_eq!(get_flag(wrpc, ()).await?, Bar::BAZ);
    Ok(())
}
