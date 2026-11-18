//@ args = '--enable-method-chaining'

use crate::client::foo::bar::i::A;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (mut tx, mut rx) = wrpc.connect().await?;
    let my_a = A::new(&mut tx, &mut rx).await?;
    let my_a = my_a.as_borrow();
    A::set_a(&mut tx, &mut rx, &my_a, 42).await?;
    A::set_b(&mut tx, &mut rx, &my_a, true).await?;
    A::do_(&mut tx, &mut rx, &my_a).await?;
    Ok(())
}
