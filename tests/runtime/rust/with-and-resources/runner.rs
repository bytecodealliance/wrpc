//@ args = '--with my:inline/foo=other::my::inline::foo'

mod other {
    ::wit_bindgen_wrpc::generate!({
        inline: "
            package my:inline;

            interface foo {
                resource a;

                bar: func() -> a;
            }

            world dummy {
                import foo;
            }
        ",
        generate_all,
    });
}

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let resource = other::my::inline::foo::bar(wrpc, ()).await?;
    let _ = crate::client::my::inline::bar::bar(wrpc, (), &resource).await?;
    Ok(())
}
