//@ args = '--with my:inline/foo=other::my::inline::foo'

use core::any::TypeId;

use ::wit_bindgen_wrpc::wrpc_transport::Invoke;

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
pub struct Component;

impl<C: Invoke<Context = ()>> Handler<C> for Component {
    async fn bar(&self, cx: C, a: A) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        assert!(TypeId::of::<A>() != TypeId::of::<other::my::inline::foo::A>());
        other::my::inline::foo::bar(&cx, (), &other::my::inline::foo::A { b: a.b }).await
    }
}
