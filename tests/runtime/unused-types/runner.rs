//@ args = '--generate-unused-types'

#[expect(unused_imports)]
use crate::client::foo::bar::component::UnusedEnum as _;
#[expect(unused_imports)]
use crate::client::foo::bar::component::UnusedRecord as _;
#[expect(unused_imports)]
use crate::client::foo::bar::component::UnusedVariant as _;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    crate::client::foo::bar::component::foo(tx, rx).await?;
    Ok(())
}
