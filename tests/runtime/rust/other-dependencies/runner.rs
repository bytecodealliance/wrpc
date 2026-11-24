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
) -> ::anyhow::Result<()> {
    let clt = wrpc.connect().await?;
    other::test::test::f(clt).await?;
    Ok(())
}
