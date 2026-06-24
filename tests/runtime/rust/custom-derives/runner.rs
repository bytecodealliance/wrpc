use crate::client::my::inline::blah::{bar, Foo};

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    bar(
        wrpc,
        (),
        &Foo {
            field1: "x".to_string(),
            field2: vec![2, 3, 3, 4],
        },
    )
    .await?;
    Ok(())
}
