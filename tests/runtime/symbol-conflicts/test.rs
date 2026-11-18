#[derive(Clone)]
pub struct Component(pub crate::Client);

impl crate::server::exports::my::inline::foo1::Handler for Component {
    async fn foo(&self) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}

impl crate::server::exports::my::inline::foo2::Handler for Component {
    async fn foo(&self) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }
}

impl crate::server::exports::my::inline::bar1::Handler for Component {
    async fn bar(&self) -> ::wit_bindgen_wrpc::anyhow::Result<String> {
        Ok(String::new())
    }
}

impl crate::server::exports::my::inline::bar2::Handler for Component {
    async fn bar(&self) -> ::wit_bindgen_wrpc::anyhow::Result<String> {
        Ok(String::new())
    }
}
