use crate::client::test::flavorful::to_test::*;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    f_list_in_record1(
        wrpc,
        (),
        &ListInRecord1 {
            a: "list_in_record1".to_string(),
        },
    )
    .await?;
    assert_eq!(f_list_in_record2(wrpc, ()).await?.a, "list_in_record2");

    assert_eq!(
        f_list_in_record3(
            wrpc,
            (),
            &ListInRecord3 {
                a: "list_in_record3 input".to_string()
            }
        )
        .await?
        .a,
        "list_in_record3 output"
    );

    assert_eq!(
        f_list_in_record4(
            wrpc,
            (),
            &ListInAlias {
                a: "input4".to_string()
            }
        )
        .await?
        .a,
        "result4"
    );

    f_list_in_variant1(wrpc, (), Some("foo"), &Err("bar".to_string())).await?;
    assert_eq!(
        f_list_in_variant2(wrpc, ()).await?,
        Some("list_in_variant2".to_string())
    );
    assert_eq!(
        f_list_in_variant3(wrpc, (), Some("input3")).await?,
        Some("output3".to_string())
    );

    assert!(errno_result(wrpc, ()).await?.is_err());
    MyErrno::A.to_string();
    _ = format!("{:?}", MyErrno::A);
    fn assert_error<T: std::error::Error>() {}
    assert_error::<MyErrno>();

    assert!(errno_result(wrpc, ()).await?.is_ok());

    let (a, b) = list_typedefs(wrpc, (), "typedef1", &["typedef2"]).await?;
    assert_eq!(a.as_ref(), b"typedef3");
    assert_eq!(b.len(), 1);
    assert_eq!(b[0], "typedef4");

    let (a, b, c) = list_of_variants(
        wrpc,
        (),
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
