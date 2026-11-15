use crate::client::test::flavorful::to_test::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    f_list_in_record1(
        tx,
        rx,
        &ListInRecord1 {
            a: "list_in_record1".to_string(),
        },
    )
    .await?;
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(f_list_in_record2(tx, rx).await?.a, "list_in_record2");

    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        f_list_in_record3(
            tx,
            rx,
            &ListInRecord3 {
                a: "list_in_record3 input".to_string()
            }
        )
        .await?
        .a,
        "list_in_record3 output"
    );

    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        f_list_in_record4(
            tx,
            rx,
            &ListInAlias {
                a: "input4".to_string()
            }
        )
        .await?
        .a,
        "result4"
    );

    let (tx, rx) = wrpc.connect().await?;
    f_list_in_variant1(tx, rx, Some("foo"), &Err("bar".to_string())).await?;
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        f_list_in_variant2(tx, rx).await?,
        Some("list_in_variant2".to_string())
    );
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        f_list_in_variant3(tx, rx, Some("input3")).await?,
        Some("output3".to_string())
    );

    let (tx, rx) = wrpc.connect().await?;
    assert!(errno_result(tx, rx).await?.is_err());
    MyErrno::A.to_string();
    _ = format!("{:?}", MyErrno::A);
    fn assert_error<T: std::error::Error>() {}
    assert_error::<MyErrno>();

    let (tx, rx) = wrpc.connect().await?;
    assert!(errno_result(tx, rx).await?.is_ok());

    let (tx, rx) = wrpc.connect().await?;
    let (a, b) = list_typedefs(tx, rx, "typedef1", &["typedef2"]).await?;
    assert_eq!(a.as_ref(), b"typedef3");
    assert_eq!(b.len(), 1);
    assert_eq!(b[0], "typedef4");

    let (tx, rx) = wrpc.connect().await?;
    let (a, b, c) = list_of_variants(
        tx,
        rx,
        &[true, false],
        &[Ok(()), Err(())],
        &[MyErrno::Success, MyErrno::A],
    )
    .await?;
    assert_eq!(a, [false, true]);
    assert_eq!(b, [Err(()), Ok(())]);
    assert_eq!(c, [MyErrno::A, MyErrno::B]);
    Ok(())
}
