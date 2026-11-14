use ::wit_bindgen_wrpc::wrpc_transport::Connect;

use crate::server::exports::test::results::test as test_exports;
use crate::server::test::results::test as test_imports;

#[derive(Clone)]
pub struct Component;

impl<C: Connect<Context = ()>> test_exports::Handler<C> for Component {
    async fn string_error(
        &self,
        cx: C,
        a: f32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Result<f32, String>> {
        test_imports::string_error(&cx, (), a).await
    }

    async fn enum_error(
        &self,
        cx: C,
        a: f32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Result<f32, test_exports::E>> {
        let res = test_imports::enum_error(&cx, (), a).await?;
        Ok(match res {
            Ok(b) => Ok(b),
            Err(test_imports::E::A) => Err(test_exports::E::A),
            Err(test_imports::E::B) => Err(test_exports::E::B),
            Err(test_imports::E::C) => Err(test_exports::E::C),
        })
    }

    async fn record_error(
        &self,
        cx: C,
        a: f32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Result<f32, test_exports::E2>> {
        let res = test_imports::record_error(&cx, (), a).await?;
        Ok(match res {
            Ok(b) => Ok(b),
            Err(test_imports::E2 { line, column }) => Err(test_exports::E2 { line, column }),
        })
    }

    async fn variant_error(
        &self,
        cx: C,
        a: f32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Result<f32, test_exports::E3>> {
        let res = test_imports::variant_error(&cx, (), a).await?;
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
        cx: C,
        a: u32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Result<u32, ()>> {
        test_imports::empty_error(&cx, (), a).await
    }

    async fn double_error(
        &self,
        cx: C,
        a: u32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Result<Result<(), String>, String>> {
        test_imports::double_error(&cx, (), a).await
    }
}
