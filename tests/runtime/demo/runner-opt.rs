//@ [lang]
//@ rustflags = '-O'

use crate::client::a::b::the_test::x;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let clt = wrpc.connect().await?;
    x(clt).await
}
