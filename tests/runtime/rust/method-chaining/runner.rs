//@ args = '--enable-method-chaining'

use crate::client::foo::bar::i::A;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    let my_a = A::new(tx, rx).await?;
    let my_a = my_a.as_borrow();
    let (tx, rx) = wrpc.connect().await?;
    A::set_a(tx, rx, &my_a, 42).await?;
    let (tx, rx) = wrpc.connect().await?;
    A::set_b(tx, rx, &my_a, true).await?;
    let (tx, rx) = wrpc.connect().await?;
    A::do_(tx, rx, &my_a).await?;
    Ok(())
}
