use crate::client::my::inline::foo::Bar;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let mut clt = wrpc.connect().await?;
    let data = Bar::new(&mut clt, 3).await?;
    assert_eq!(Bar::get_data(&mut clt, &data.as_borrow()).await?, 3);
    assert_eq!(Bar::consume(&mut clt, &data).await?, 4);
    Ok(())
}
