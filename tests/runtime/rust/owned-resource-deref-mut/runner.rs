use crate::client::my::inline::foo::Bar;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (mut tx, mut rx) = wrpc.connect().await?;
    let data = Bar::new(&mut tx, &mut rx, 3).await?;
    assert_eq!(Bar::get_data(&mut tx, &mut rx, &data.as_borrow()).await?, 3);
    assert_eq!(Bar::consume(&mut tx, &mut rx, &data).await?, 4);
    Ok(())
}
