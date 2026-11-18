
use crate::server::exports::test::results::test as test_exports;
use crate::server::test::results::test as test_imports;

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl test_exports::Handler for Component {
    async fn string_error(
        &self,
        a: f32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Result<f32, String>> {
        let (tx, rx) = self.0.connect().await?;
        test_imports::string_error(tx, rx, a).await
    }

    async fn enum_error(
        &self,
        a: f32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Result<f32, test_exports::E>> {
        let (tx, rx) = self.0.connect().await?;
        let res = test_imports::enum_error(tx, rx, a).await?;
        Ok(match res {
            Ok(b) => Ok(b),
            Err(test_imports::E::A) => Err(test_exports::E::A),
            Err(test_imports::E::B) => Err(test_exports::E::B),
            Err(test_imports::E::C) => Err(test_exports::E::C),
        })
    }

    async fn record_error(
        &self,
        a: f32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Result<f32, test_exports::E2>> {
        let (tx, rx) = self.0.connect().await?;
        let res = test_imports::record_error(tx, rx, a).await?;
        Ok(match res {
            Ok(b) => Ok(b),
            Err(test_imports::E2 { line, column }) => Err(test_exports::E2 { line, column }),
        })
    }

    async fn variant_error(
        &self,
        a: f32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Result<f32, test_exports::E3>> {
        let (tx, rx) = self.0.connect().await?;
        let res = test_imports::variant_error(tx, rx, a).await?;
        Ok(match res {
            Ok(b) => Ok(b),
            Err(test_imports::E3::E1(test_imports::E::A)) => {
                Err(test_exports::E3::E1(test_exports::E::A))
            }
            Err(test_imports::E3::E1(test_imports::E::B)) => {
                Err(test_exports::E3::E1(test_exports::E::B))
            }
            Err(test_imports::E3::E1(test_imports::E::C)) => {
                Err(test_exports::E3::E1(test_exports::E::C))
            }
            Err(test_imports::E3::E2(test_imports::E2 { line, column })) => {
                Err(test_exports::E3::E2(test_exports::E2 { line, column }))
            }
        })
    }

    async fn empty_error(
        &self,
        a: u32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Result<u32, ()>> {
        let (tx, rx) = self.0.connect().await?;
        test_imports::empty_error(tx, rx, a).await
    }

    async fn double_error(
        &self,
        a: u32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Result<Result<(), String>, String>> {
        let (tx, rx) = self.0.connect().await?;
        test_imports::double_error(tx, rx, a).await
    }
}
