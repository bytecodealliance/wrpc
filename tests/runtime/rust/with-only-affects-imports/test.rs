//@ args = '--with my:inline/foo=other::my::inline::foo'

use core::any::TypeId;

use crate::server::exports::my::inline::foo::{A, Handler};

mod other {
    ::wit_bindgen_wrpc::generate!({
        inline: "
            package my:inline;

            interface foo {
                record a { b: u8 }

                bar: func(a: a);
            }

            world gen {
                import foo;
            }
        ",
    });
}

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn bar(&self, a: A) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        assert!(TypeId::of::<A>() != TypeId::of::<other::my::inline::foo::A>());
        let (tx, rx) = self.0.connect().await?;
        other::my::inline::foo::bar(tx, rx, &other::my::inline::foo::A { b: a.b }).await
    }
}
