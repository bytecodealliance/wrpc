pub async fn run(
    wrpc: &crate::Client,
) -> ::anyhow::Result<()> {
    let clt = wrpc.connect().await?;
    crate::client::test::many_arguments::to_test::many_arguments(
        clt,
        1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16,
    )
    .await?;
    Ok(())
}
