use crate::client::my::inline::foo::Bar;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    let data = Bar::new(tx, rx, 3).await?;
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(Bar::get_data(tx, rx, &data.as_borrow()).await?, 3);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(Bar::consume(tx, rx, &data).await?, 4);
    Ok(())
}
