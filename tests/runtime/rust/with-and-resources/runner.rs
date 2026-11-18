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
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (mut tx, mut rx) = wrpc.connect().await?;
    let resource = other::my::inline::foo::bar(&mut tx, &mut rx).await?;
    let _ = crate::client::my::inline::bar::bar(&mut tx, &mut rx, &resource).await?;
    Ok(())
}
