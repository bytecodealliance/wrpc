#[derive(Clone)]
pub struct Component(pub crate::Client);

impl crate::server::exports::test::strings::to_test::Handler for Component {
    async fn take_basic(&self, s: String) -> ::std::io::Result<()> {
        assert_eq!(s, "latin utf16");
        Ok(())
    }

    async fn return_unicode(&self) -> ::std::io::Result<String> {
        Ok("🚀🚀🚀 𠈄𓀀".to_string())
    }

    async fn return_empty(&self) -> ::std::io::Result<String> {
        Ok("".to_string())
    }

    async fn roundtrip(&self, s: String) -> ::std::io::Result<String> {
        Ok(s.clone())
    }
}
