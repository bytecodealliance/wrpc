//@ args = '--generate-unused-types'

#[expect(unused_imports)]
use crate::client::foo::bar::component::UnusedEnum as _;
#[expect(unused_imports)]
use crate::client::foo::bar::component::UnusedRecord as _;
#[expect(unused_imports)]
use crate::client::foo::bar::component::UnusedVariant as _;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    crate::client::foo::bar::component::foo(wrpc, ()).await?;
    Ok(())
}
