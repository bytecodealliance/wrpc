use crate::client::test::variants::to_test::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (mut tx, mut rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_option(&mut tx, &mut rx, Some(1.0)).await?, Some(1));
    assert_eq!(roundtrip_option(&mut tx, &mut rx, None).await?, None);
    assert_eq!(roundtrip_option(&mut tx, &mut rx, Some(2.0)).await?, Some(2));
    assert_eq!(roundtrip_result(&mut tx, &mut rx, &Ok(2)).await?, Ok(2.0));
    assert_eq!(roundtrip_result(&mut tx, &mut rx, &Ok(4)).await?, Ok(4.0));
    assert_eq!(roundtrip_result(&mut tx, &mut rx, &Err(5.3)).await?, Err(5));

    assert_eq!(roundtrip_enum(&mut tx, &mut rx, E1::A).await?, E1::A);
    assert_eq!(roundtrip_enum(&mut tx, &mut rx, E1::B).await?, E1::B);

    assert_eq!(invert_bool(&mut tx, &mut rx, true).await?, false);
    assert_eq!(invert_bool(&mut tx, &mut rx, false).await?, true);

    let (a1, a2, a3, a4, a5, a6) = variant_casts(
        &mut tx,
        &mut rx,
        (C1::A(1), C2::A(2), C3::A(3), C4::A(4), C5::A(5), C6::A(6.0)),
    )
    .await?;
    assert!(matches!(a1, C1::A(1)));
    assert!(matches!(a2, C2::A(2)));
    assert!(matches!(a3, C3::A(3)));
    assert!(matches!(a4, C4::A(4)));
    assert!(matches!(a5, C5::A(5)));
    assert!(matches!(a6, C6::A(b) if b == 6.0));

    let (a1, a2, a3, a4, a5, a6) = variant_casts(
        &mut tx,
        &mut rx,
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

    let (a1, a2, a3, a4) =
        variant_zeros(&mut tx, &mut rx, (Z1::A(1), Z2::A(2), Z3::A(3.0), Z4::A(4.0))).await?;
    assert!(matches!(a1, Z1::A(1)));
    assert!(matches!(a2, Z2::A(2)));
    assert!(matches!(a3, Z3::A(b) if b == 3.0));
    assert!(matches!(a4, Z4::A(b) if b == 4.0));

    let (a1, a2, a3, a4) = variant_zeros(&mut tx, &mut rx, (Z1::B, Z2::B, Z3::B, Z4::B)).await?;
    assert!(matches!(a1, Z1::B));
    assert!(matches!(a2, Z2::B));
    assert!(matches!(a3, Z3::B));
    assert!(matches!(a4, Z4::B));

    variant_typedefs(&mut tx, &mut rx, None, false, &Err(())).await?;

    assert_eq!(
        variant_enums(&mut tx, &mut rx, true, &Ok(()), MyErrno::Success).await?,
        (true, Ok(()), MyErrno::Success)
    );
    Ok(())
}
