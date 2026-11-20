pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let clt = wrpc.connect().await?;
    crate::client::my::inline::foo::bar(clt, &crate::client::my::inline::foo::A { b: 2 }).await
}
