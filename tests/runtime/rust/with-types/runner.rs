//@ args = [
//@     '--with=my:inline/foo/a=crate::runner::my_types::MyA',
//@     '--with=my:inline/foo/b=crate::runner::my_types::MyB',
//@     '--with=my:inline/foo/c=crate::runner::my_types::MyC',
//@     '--with=d=crate::runner::my_types::MyD',
//@     '--with=my:inline/bar/e=crate::runner::my_types::MyE',
//@     '--with=my:inline/foo/f=generate',
//@ ]

mod my_types {
    ::wit_bindgen_wrpc::generate!({
        inline: "
            package my:types;

            interface t {
                record a {
                    inner: f64,
                }

                resource b;

                variant c {
                    a(a),
                    b(b),
                }

                record d {
                    inner: u32,
                }

                record e {
                    inner: u32,
                }

                use-a: func(v: a) -> a;
                use-b: func(v: b) -> b;
                use-c: func(v: c) -> c;
                use-d: func(v: d) -> d;
                use-e: func(v: e) -> e;
            }

            world dummy {
                import t;
            }
        ",
        generate_all,
    });

    pub use self::my::types::t::{A as MyA, B as MyB, C as MyC, D as MyD, E as MyE};
}

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let a = my_types::MyA { inner: 0.0 };
    let _ = crate::runner::my::inline::foo::func1(wrpc, (), &a).await?;

    // can't actually succeed at runtime as this is faking a resource, so check
    // that it compiles but dynamically skip it.
    if false {
        let b = ::wit_bindgen_wrpc::wrpc_transport::ResourceOwn::<my_types::MyB>::from(
            ::wit_bindgen_wrpc::bytes::Bytes::new(),
        );
        let _ = crate::runner::my::inline::foo::func2(wrpc, (), &b).await?;
    }

    let c = my_types::MyC::A(a);
    let _ = crate::runner::i::func7(wrpc, (), &c).await?;

    let a_list = vec![a, a];
    let _ = crate::runner::my::inline::foo::func3(wrpc, (), &a_list).await?;

    let _ = crate::runner::my::inline::foo::func4(wrpc, (), Some(a)).await?;

    let _ = crate::runner::my::inline::foo::func5(wrpc, ()).await?;

    let d = my_types::MyD { inner: 0 };
    let _ = crate::runner::i::func8(wrpc, (), &d).await?;
    Ok(())
}
