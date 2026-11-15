use crate::client::test::results::test::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    let v = string_error(tx, rx, 0.0).await?;
    assert_eq!(v, Err("zero".to_owned()));
    let (tx, rx) = wrpc.connect().await?;
    let v = string_error(tx, rx, 1.0).await?;
    assert_eq!(v, Ok(1.0));

    let (tx, rx) = wrpc.connect().await?;
    let v = enum_error(tx, rx, 0.0).await?;
    assert_eq!(v, Err(E::A));
    let (tx, rx) = wrpc.connect().await?;
    let v = enum_error(tx, rx, 1.0).await?;
    assert_eq!(v, Ok(1.0));

    let (tx, rx) = wrpc.connect().await?;
    let v = record_error(tx, rx, 0.0).await?;
    assert!(matches!(
        v,
        Err(E2 {
            line: 420,
            column: 0
        })
    ));
    let (tx, rx) = wrpc.connect().await?;
    let v = record_error(tx, rx, 1.0).await?;
    assert!(matches!(
        v,
        Err(E2 {
            line: 77,
            column: 2
        })
    ));
    let (tx, rx) = wrpc.connect().await?;
    let v = record_error(tx, rx, 2.0).await?;
    assert!(v.is_ok());

    let (tx, rx) = wrpc.connect().await?;
    let v = variant_error(tx, rx, 0.0).await?;
    assert!(matches!(
        v,
        Err(E3::E2(E2 {
            line: 420,
            column: 0
        }))
    ));
    let (tx, rx) = wrpc.connect().await?;
    let v = variant_error(tx, rx, 1.0).await?;
    assert!(matches!(v, Err(E3::E1(E::B))));
    let (tx, rx) = wrpc.connect().await?;
    let v = variant_error(tx, rx, 2.0).await?;
    assert!(matches!(v, Err(E3::E1(E::C))));

    let (tx, rx) = wrpc.connect().await?;
    let v = empty_error(tx, rx, 0).await?;
    assert_eq!(v, Err(()));
    let (tx, rx) = wrpc.connect().await?;
    let v = empty_error(tx, rx, 1).await?;
    assert_eq!(v, Ok(42));
    let (tx, rx) = wrpc.connect().await?;
    let v = empty_error(tx, rx, 2).await?;
    assert_eq!(v, Ok(2));

    let (tx, rx) = wrpc.connect().await?;
    let v = double_error(tx, rx, 0).await?;
    assert_eq!(v, Ok(Ok(())));
    let (tx, rx) = wrpc.connect().await?;
    let v = double_error(tx, rx, 1).await?;
    assert_eq!(v, Ok(Err("one".into())));
    let (tx, rx) = wrpc.connect().await?;
    let v = double_error(tx, rx, 2).await?;
    assert_eq!(v, Err("two".into()));
    Ok(())
}
