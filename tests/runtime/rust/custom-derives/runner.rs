use my::inline::blah::{bar, Foo};

pub async fn run(
    clt: &impl wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> anyhow::Result<()> {
    bar(
        clt,
        (),
        &Foo {
            field1: "x".to_string(),
            field2: vec![2, 3, 3, 4],
        },
    )
    .await?;
    Ok(())
}
