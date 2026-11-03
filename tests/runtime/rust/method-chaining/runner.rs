//@ args = '--enable-method-chaining'

use crate::client::foo::bar::i::A;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let my_a = A::new(wrpc, ()).await?;
    let my_a = my_a.as_borrow();
    A::set_a(wrpc, (), &my_a, 42).await?;
    A::set_b(wrpc, (), &my_a, true).await?;
    A::do_(wrpc, (), &my_a).await?;
    Ok(())
}
