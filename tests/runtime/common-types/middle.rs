
use crate::server::exports::test::common::to_test;
use crate::server::test::common::to_test::{F1, R1, V1, var_f, wrap};

#[derive(Clone)]
pub struct Component;

impl to_test::Handler<crate::Client> for Component {
    async fn wrap(&self, cx: crate::Client, flag: F1) -> ::wit_bindgen_wrpc::anyhow::Result<R1> {
        let (tx, rx) = cx.connect().await?;
        wrap(tx, rx, &flag).await
    }

    async fn var_f(&self, cx: crate::Client) -> ::wit_bindgen_wrpc::anyhow::Result<V1> {
        let (tx, rx) = cx.connect().await?;
        var_f(tx, rx).await
    }
}
