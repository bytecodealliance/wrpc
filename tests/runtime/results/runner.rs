use crate::client::test::results::test::*;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Connect<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let v = string_error(wrpc, (), 0.0).await?;
    assert_eq!(v, Err("zero".to_owned()));
    let v = string_error(wrpc, (), 1.0).await?;
    assert_eq!(v, Ok(1.0));

    let v = enum_error(wrpc, (), 0.0).await?;
    assert_eq!(v, Err(E::A));
    let v = enum_error(wrpc, (), 1.0).await?;
    assert_eq!(v, Ok(1.0));

    let v = record_error(wrpc, (), 0.0).await?;
    assert!(matches!(
        v,
        Err(E2 {
            line: 420,
            column: 0
        })
    ));
    let v = record_error(wrpc, (), 1.0).await?;
    assert!(matches!(
        v,
        Err(E2 {
            line: 77,
            column: 2
        })
    ));
    let v = record_error(wrpc, (), 2.0).await?;
    assert!(v.is_ok());

    let v = variant_error(wrpc, (), 0.0).await?;
    assert!(matches!(
        v,
        Err(E3::E2(E2 {
            line: 420,
            column: 0
        }))
    ));
    let v = variant_error(wrpc, (), 1.0).await?;
    assert!(matches!(v, Err(E3::E1(E::B))));
    let v = variant_error(wrpc, (), 2.0).await?;
    assert!(matches!(v, Err(E3::E1(E::C))));

    let v = empty_error(wrpc, (), 0).await?;
    assert_eq!(v, Err(()));
    let v = empty_error(wrpc, (), 1).await?;
    assert_eq!(v, Ok(42));
    let v = empty_error(wrpc, (), 2).await?;
    assert_eq!(v, Ok(2));

    let v = double_error(wrpc, (), 0).await?;
    assert_eq!(v, Ok(Ok(())));
    let v = double_error(wrpc, (), 1).await?;
    assert_eq!(v, Ok(Err("one".into())));
    let v = double_error(wrpc, (), 2).await?;
    assert_eq!(v, Err("two".into()));
    Ok(())
}
