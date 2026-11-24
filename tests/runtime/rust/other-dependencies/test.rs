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
pub struct Component(pub crate::Client);

impl Handler for Component {
    async fn f(&self) -> ::std::io::Result<()> {
        Ok(())
    }
}
