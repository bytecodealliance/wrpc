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
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Connect<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    other::test::test::f(wrpc, ()).await
}
