use ::wit_bindgen_wrpc::wrpc_transport::Invoke;

use crate::server::exports::test::common::to_test;
use crate::server::test::common::to_test::{F1, R1, V1, var_f, wrap};

#[derive(Clone)]
pub struct Component;

impl<C: Invoke<Context = ()>> to_test::Handler<C> for Component {
    async fn wrap(&self, cx: C, flag: F1) -> ::wit_bindgen_wrpc::anyhow::Result<R1> {
        wrap(&cx, (), &flag).await
    }

    async fn var_f(&self, cx: C) -> ::wit_bindgen_wrpc::anyhow::Result<V1> {
        var_f(&cx, ()).await
    }
}
