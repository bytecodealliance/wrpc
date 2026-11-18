
use crate::server::exports::test::common::to_test;
use crate::server::test::common::to_test::{F1, R1, V1, var_f, wrap};

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl to_test::Handler for Component {
    async fn wrap(&self, flag: F1) -> ::wit_bindgen_wrpc::anyhow::Result<R1> {
        let (tx, rx) = self.0.connect().await?;
        wrap(tx, rx, &flag).await
    }

    async fn var_f(&self) -> ::wit_bindgen_wrpc::anyhow::Result<V1> {
        let (tx, rx) = self.0.connect().await?;
        var_f(tx, rx).await
    }
}
