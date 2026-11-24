//@ args = '--generate-unused-types'

#[expect(unused_imports)]
use crate::client::foo::bar::component::UnusedEnum as _;
#[expect(unused_imports)]
use crate::client::foo::bar::component::UnusedRecord as _;
#[expect(unused_imports)]
use crate::client::foo::bar::component::UnusedVariant as _;

pub async fn run(
    wrpc: &crate::Client,
) -> ::anyhow::Result<()> {
    let clt = wrpc.connect().await?;
    crate::client::foo::bar::component::foo(clt).await?;
    Ok(())
}
