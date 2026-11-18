#[derive(Clone)]
pub struct Component(pub crate::Client);

impl crate::server::exports::test::dep0_1_0::test::Handler for Component {
    async fn x(&self) -> ::wit_bindgen_wrpc::anyhow::Result<f32> {
        Ok(1.0)
    }

    async fn y(&self, a: f32) -> ::wit_bindgen_wrpc::anyhow::Result<f32> {
        Ok(1.0 + a)
    }
}

impl crate::server::exports::test::dep0_2_0::test::Handler for Component {
    async fn x(&self) -> ::wit_bindgen_wrpc::anyhow::Result<f32> {
        Ok(2.0)
    }

    async fn z(&self, a: f32, b: f32) -> ::wit_bindgen_wrpc::anyhow::Result<f32> {
        Ok(2.0 + a + b)
    }
}
