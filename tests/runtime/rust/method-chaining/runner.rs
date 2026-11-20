//@ args = '--enable-method-chaining'

use crate::client::foo::bar::i::A;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let mut clt = wrpc.connect().await?;
    let my_a = A::new(&mut clt).await?;
    let my_a = my_a.as_borrow();
    A::set_a(&mut clt, &my_a, 42).await?;
    A::set_b(&mut clt, &my_a, true).await?;
    A::do_(&mut clt, &my_a).await?;
    Ok(())
}
