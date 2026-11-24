//@ [lang]
//@ rustflags = '-O'

use crate::client::a::b::the_test::x;

pub async fn run(
    wrpc: &crate::Client,
) -> ::anyhow::Result<()> {
    let clt = wrpc.connect().await?;
    x(clt).await?;
    Ok(())
}
