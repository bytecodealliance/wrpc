::wit_bindgen_wrpc::generate!({
    inline: r#"
        package test:deps;

        world test {
            import other:test/test;
            export run: func();
        }
    "#,
    path: "./other.wit",
    with: {
        "other:test/test": generate,
    }
});

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    other::test::test::f(tx, rx).await
}
