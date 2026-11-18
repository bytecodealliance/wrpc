#[derive(Clone)]
pub struct Component(pub crate::Client);

impl crate::server::exports::my::inline::bar::Handler for Component {
    async fn bar(
        &self,
        m: crate::server::exports::my::inline::bar::Msg,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        assert_eq!(m.field, "hello");
        Ok(())
    }
}
