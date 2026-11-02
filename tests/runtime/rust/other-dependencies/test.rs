::wit_bindgen_wrpc::generate!({
    inline: r#"
        package test:deps;

        world test {
            export other:test/test;
        }
    "#,
    path: "./other.wit",
    with: {
        "other:test/test": generate,
    }
});

use crate::server::exports::other::test::test::Handler;

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> Handler<Ctx> for Component {
    async fn f(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}
