//@ args = '--with my:inline/foo=other::my::inline::foo'

mod other {
    ::wit_bindgen_wrpc::generate!({
        inline: "
            package my:inline;

            interface foo {
                record msg {
                    field: string,
                }
            }

            world dummy {
                use foo.{msg};
                import bar: func(m: msg);
            }
        ",
        generate_all,
    });
}

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let msg = other::my::inline::foo::Msg {
        field: "hello".to_string(),
    };
    crate::client::my::inline::bar::bar(wrpc, (), &msg).await?;
    Ok(())
}
