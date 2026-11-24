//@ args = '--bitflags-path crate::client::my_bitflags'

pub(crate) use ::wit_bindgen_wrpc::bitflags as my_bitflags;

use crate::client::my::inline::t::{get_flag, Bar};

pub async fn run(
    wrpc: &crate::Client,
) -> ::anyhow::Result<()> {
    let clt = wrpc.connect().await?;
    assert_eq!(get_flag(clt).await?, Bar::BAZ);
    Ok(())
}
