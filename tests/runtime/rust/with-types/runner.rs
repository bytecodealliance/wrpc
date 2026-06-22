//@ args = [
//@   '--with=my:inline/foo/a=crate::runner::other::my::inline::foo::A',
//@   '--with=my:inline/foo/c=crate::runner::other::my::inline::foo::C',
//@ ]

mod other {
    wit_bindgen_wrpc::generate!({
        inline: "
            package my:inline;
            interface foo {
                record a { inner: f64, }
                variant c { a(a), other(u32), }
            }
            world dummy {
                use foo.{a, c};
                import f: func(v: a, w: c);
            }
        ",
    });
}

pub async fn run(
    clt: &impl wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> anyhow::Result<()> {
    let a = other::my::inline::foo::A { inner: 1.5 };
    let got = my::inline::foo::func1(clt, (), &a).await?;
    assert_eq!(got.inner, 1.5);

    let c = other::my::inline::foo::C::A(a);
    my::inline::foo::func3(clt, (), &c).await?;
    Ok(())
}
