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
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Connect<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    a::test::suffix::imports::foo(wrpc, ()).await?;
    crate::client::foo::f(wrpc, ()).await?;
    crate::client::bar::f(wrpc, ()).await?;
    Ok(())
}
