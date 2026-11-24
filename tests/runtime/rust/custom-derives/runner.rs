use crate::client::my::inline::blah::{bar, Foo};

pub async fn run(
    wrpc: &crate::Client,
) -> ::anyhow::Result<()> {
    let clt = wrpc.connect().await?;
    bar(
        clt,
        &Foo {
            field1: "x".to_string(),
            field2: vec![2, 3, 3, 4],
        },
    )
    .await?;
    Ok(())
}
