use crate::client::test::common::test_types::{F1, V1};
use crate::client::test::common::to_test::{var_f, wrap};

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Connect<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let res = wrap(wrpc, (), &F1::A).await?;
    assert_eq!(res.b, F1::A);
    assert_eq!(res.a, 1);

    let res2 = wrap(wrpc, (), &F1::B).await?;
    assert_eq!(res2.b, F1::B);
    assert_eq!(res2.a, 2);

    let res3 = var_f(wrpc, ()).await?;
    assert!(matches!(res3, V1::B(42)));
    Ok(())
}
