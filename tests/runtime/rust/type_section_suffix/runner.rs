mod a {
    ::wit_bindgen_wrpc::generate!({
        world: "available-imports",
        path: "./test.wit",
        generate_all,
    });
}

mod c {
    ::wit_bindgen_wrpc::generate!({
        world: "test:a/imports",
        path: "./test.wit",
    });
}
mod d {
    ::wit_bindgen_wrpc::generate!({
        world: "test:b/imports",
        path: "./test.wit",
    });
}

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    a::test::suffix::imports::foo(tx, rx).await?;
    let (tx, rx) = wrpc.connect().await?;
    crate::client::foo::f(tx, rx).await?;
    let (tx, rx) = wrpc.connect().await?;
    crate::client::bar::f(tx, rx).await?;
    Ok(())
}
