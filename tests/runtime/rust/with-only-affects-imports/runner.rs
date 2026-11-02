pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    crate::client::my::inline::foo::bar(wrpc, (), &crate::client::my::inline::foo::A { b: 2 }).await
}
