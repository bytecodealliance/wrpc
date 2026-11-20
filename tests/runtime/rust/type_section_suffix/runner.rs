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
    let mut clt = wrpc.connect().await?;
    a::test::suffix::imports::foo(&mut clt).await?;
    crate::client::foo::f(&mut clt).await?;
    crate::client::bar::f(&mut clt).await?;
    Ok(())
}
