//@ args = '--bitflags-path crate::client::my_bitflags'

pub(crate) use ::wit_bindgen_wrpc::bitflags as my_bitflags;

use crate::client::my::inline::t::{get_flag, Bar};

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(get_flag(tx, rx).await?, Bar::BAZ);
    Ok(())
}
