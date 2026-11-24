#[derive(Clone)]
pub struct Component(pub crate::Client);

impl crate::server::exports::test::options::to_test::Handler for Component {
    async fn option_none_param(
        &self,
        a: Option<String>,
    ) -> ::std::io::Result<()> {
        assert!(a.is_none());
        Ok(())
    }

    async fn option_none_result(
        &self,
    ) -> ::std::io::Result<Option<String>> {
        Ok(None)
    }

    async fn option_some_param(
        &self,
        a: Option<String>,
    ) -> ::std::io::Result<()> {
        assert_eq!(a, Some("foo".to_string()));
        Ok(())
    }

    async fn option_some_result(
        &self,
    ) -> ::std::io::Result<Option<String>> {
        Ok(Some("foo".to_string()))
    }

    async fn option_roundtrip(
        &self,
        a: Option<String>,
    ) -> ::std::io::Result<Option<String>> {
        Ok(a)
    }

    async fn double_option_roundtrip(
        &self,
        a: Option<Option<u32>>,
    ) -> ::std::io::Result<Option<Option<u32>>> {
        Ok(a)
    }
}
