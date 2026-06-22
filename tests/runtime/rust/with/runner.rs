//@ args = '--with=my:inline/foo=other::my::inline::foo'

mod other {
    wit_bindgen_wrpc::generate!({
        inline: "
            package my:inline;
            interface foo {
                record msg { field: string, }
            }
            world dummy {
                use foo.{msg};
                import bar: func(m: msg);
            }
        ",
    });
}

pub async fn run(
    clt: &impl wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> anyhow::Result<()> {
    let msg = other::my::inline::foo::Msg {
        field: "hello".to_string(),
    };
    my::inline::bar::bar(clt, (), &msg).await?;
    Ok(())
}
