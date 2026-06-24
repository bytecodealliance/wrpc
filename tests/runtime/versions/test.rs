#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> crate::server::exports::test::dep0_1_0::test::Handler<Ctx> for Component {
    async fn x(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<f32> {
        Ok(1.0)
    }

    async fn y(&self, _cx: Ctx, a: f32) -> ::wit_bindgen_wrpc::anyhow::Result<f32> {
        Ok(1.0 + a)
    }
}

impl<Ctx: Send> crate::server::exports::test::dep0_2_0::test::Handler<Ctx> for Component {
    async fn x(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<f32> {
        Ok(2.0)
    }

    async fn z(&self, _cx: Ctx, a: f32, b: f32) -> ::wit_bindgen_wrpc::anyhow::Result<f32> {
        Ok(2.0 + a + b)
    }
}
