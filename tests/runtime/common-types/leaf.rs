use crate::leaf::exports::test::common::to_test::Handler;
use crate::leaf::test::common::test_types::{F1, R1, V1};

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> Handler<Ctx> for Component {
    async fn wrap(&self, _cx: Ctx, flag: F1) -> ::wit_bindgen_wrpc::anyhow::Result<R1> {
        if flag == F1::A {
            Ok(R1 { a: 1, b: flag })
        } else {
            Ok(R1 { a: 2, b: flag })
        }
    }

    async fn var_f(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<V1> {
        Ok(V1::B(42))
    }
}
