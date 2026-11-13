//@ [lang]
//@ rustflags = '-O'

use crate::client::a::b::the_test::x;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Connect<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    x(wrpc, ()).await
}
