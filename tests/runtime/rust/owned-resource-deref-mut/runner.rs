use crate::client::my::inline::foo::Bar;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let data = Bar::new(wrpc, (), 3).await?;
    assert_eq!(Bar::get_data(wrpc, (), &data.as_borrow()).await?, 3);
    assert_eq!(Bar::consume(wrpc, (), &data).await?, 4);
    Ok(())
}
