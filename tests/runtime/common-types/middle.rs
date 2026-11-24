
use crate::server::exports::test::common::to_test;
use crate::server::test::common::to_test::{F1, R1, V1, var_f, wrap};

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl to_test::Handler for Component {
    async fn wrap(&self, flag: F1) -> ::std::io::Result<R1> {
        let clt = self.0.connect().await?;
        wrap(clt, &flag).await.map_err(std::io::Error::other)
    }

    async fn var_f(&self) -> ::std::io::Result<V1> {
        let clt = self.0.connect().await?;
        var_f(clt).await.map_err(std::io::Error::other)
    }
}
