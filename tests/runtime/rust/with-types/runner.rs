//@ args = [
//@     '--with=my:inline/foo/a=crate::client::my_types::MyA',
//@     '--with=my:inline/foo/b=crate::client::my_types::MyB',
//@     '--with=my:inline/foo/c=crate::client::my_types::MyC',
//@     '--with=d=crate::client::my_types::MyD',
//@     '--with=my:inline/bar/e=crate::client::my_types::MyE',
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
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let a = my_types::MyA { inner: 0.0 };
    let (mut tx, mut rx) = wrpc.connect().await?;
    let _ = crate::client::my::inline::foo::func1(&mut tx, &mut rx, &a).await?;

    // can't actually succeed at runtime as this is faking a resource, so check
    // that it compiles but dynamically skip it.
    if false {
        let b = ::wit_bindgen_wrpc::wrpc_transport::ResourceOwn::<my_types::MyB>::from(
            ::wit_bindgen_wrpc::bytes::Bytes::new(),
        );
        let _ = crate::client::my::inline::foo::func2(&mut tx, &mut rx, &b).await?;
    }

    let c = my_types::MyC::A(a);
    let _ = crate::client::i::func7(&mut tx, &mut rx, &c).await?;

    let a_list = vec![a, a];
    let _ = crate::client::my::inline::foo::func3(&mut tx, &mut rx, &a_list).await?;

    let _ = crate::client::my::inline::foo::func4(&mut tx, &mut rx, Some(a)).await?;

    let _ = crate::client::my::inline::foo::func5(&mut tx, &mut rx).await?;

    let d = my_types::MyD { inner: 0 };
    let _ = crate::client::i::func8(&mut tx, &mut rx, &d).await?;
    Ok(())
}
