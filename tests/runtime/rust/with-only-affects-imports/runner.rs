pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    crate::client::my::inline::foo::bar(tx, rx, &crate::client::my::inline::foo::A { b: 2 }).await
}
