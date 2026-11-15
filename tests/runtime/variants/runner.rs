use crate::client::test::variants::to_test::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_option(tx, rx, Some(1.0)).await?, Some(1));
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_option(tx, rx, None).await?, None);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_option(tx, rx, Some(2.0)).await?, Some(2));
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_result(tx, rx, &Ok(2)).await?, Ok(2.0));
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_result(tx, rx, &Ok(4)).await?, Ok(4.0));
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_result(tx, rx, &Err(5.3)).await?, Err(5));

    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_enum(tx, rx, E1::A).await?, E1::A);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_enum(tx, rx, E1::B).await?, E1::B);

    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(invert_bool(tx, rx, true).await?, false);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(invert_bool(tx, rx, false).await?, true);

    let (tx, rx) = wrpc.connect().await?;
    let (a1, a2, a3, a4, a5, a6) = variant_casts(
        tx,
        rx,
        (C1::A(1), C2::A(2), C3::A(3), C4::A(4), C5::A(5), C6::A(6.0)),
    )
    .await?;
    assert!(matches!(a1, C1::A(1)));
    assert!(matches!(a2, C2::A(2)));
    assert!(matches!(a3, C3::A(3)));
    assert!(matches!(a4, C4::A(4)));
    assert!(matches!(a5, C5::A(5)));
    assert!(matches!(a6, C6::A(b) if b == 6.0));

    let (tx, rx) = wrpc.connect().await?;
    let (a1, a2, a3, a4, a5, a6) = variant_casts(
        tx,
        rx,
        (
            C1::B(1),
            C2::B(2.0),
            C3::B(3.0),
            C4::B(4.0),
            C5::B(5.0),
            C6::B(6.0),
        ),
    )
    .await?;
    assert!(matches!(a1, C1::B(1)));
    assert!(matches!(a2, C2::B(b) if b == 2.0));
    assert!(matches!(a3, C3::B(b) if b == 3.0));
    assert!(matches!(a4, C4::B(b) if b == 4.0));
    assert!(matches!(a5, C5::B(b) if b == 5.0));
    assert!(matches!(a6, C6::B(b) if b == 6.0));

    let (tx, rx) = wrpc.connect().await?;
    let (a1, a2, a3, a4) =
        variant_zeros(tx, rx, (Z1::A(1), Z2::A(2), Z3::A(3.0), Z4::A(4.0))).await?;
    assert!(matches!(a1, Z1::A(1)));
    assert!(matches!(a2, Z2::A(2)));
    assert!(matches!(a3, Z3::A(b) if b == 3.0));
    assert!(matches!(a4, Z4::A(b) if b == 4.0));

    let (tx, rx) = wrpc.connect().await?;
    let (a1, a2, a3, a4) = variant_zeros(tx, rx, (Z1::B, Z2::B, Z3::B, Z4::B)).await?;
    assert!(matches!(a1, Z1::B));
    assert!(matches!(a2, Z2::B));
    assert!(matches!(a3, Z3::B));
    assert!(matches!(a4, Z4::B));

    let (tx, rx) = wrpc.connect().await?;
    variant_typedefs(tx, rx, None, false, &Err(())).await?;

    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        variant_enums(tx, rx, true, &Ok(()), MyErrno::Success).await?,
        (true, Ok(()), MyErrno::Success)
    );
    Ok(())
}
