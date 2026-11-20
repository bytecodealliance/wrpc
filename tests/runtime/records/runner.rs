use crate::client::test::records::to_test::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let mut clt = wrpc.connect().await?;
    assert_eq!(multiple_results(&mut clt).await?, (4, 5));

    assert_eq!(swap_tuple(&mut clt, (1u8, 2u32)).await?, (2u32, 1u8));
    assert_eq!(roundtrip_flags1(&mut clt, &F1::A).await?, F1::A);
    assert_eq!(roundtrip_flags1(&mut clt, &F1::empty()).await?, F1::empty());
    assert_eq!(roundtrip_flags1(&mut clt, &F1::B).await?, F1::B);
    assert_eq!(
        roundtrip_flags1(&mut clt, &(F1::A | F1::B)).await?,
        F1::A | F1::B
    );

    assert_eq!(roundtrip_flags2(&mut clt, &F2::C).await?, F2::C);
    assert_eq!(roundtrip_flags2(&mut clt, &F2::empty()).await?, F2::empty());
    assert_eq!(roundtrip_flags2(&mut clt, &F2::D).await?, F2::D);
    assert_eq!(
        roundtrip_flags2(&mut clt, &(F2::C | F2::E)).await?,
        F2::C | F2::E
    );

    assert_eq!(
        roundtrip_flags3(&mut clt, &Flag8::B0, &Flag16::B1, &Flag32::B2).await?,
        (Flag8::B0, Flag16::B1, Flag32::B2)
    );

    let r = roundtrip_record1(
        &mut clt,
        &R1 {
            a: 8,
            b: F1::empty(),
        },
    )
    .await?;
    assert_eq!(r.a, 8);
    assert_eq!(r.b, F1::empty());

    let r = roundtrip_record1(
        &mut clt,
        &R1 {
            a: 0,
            b: F1::A | F1::B,
        },
    )
    .await?;
    assert_eq!(r.a, 0);
    assert_eq!(r.b, F1::A | F1::B);

    assert_eq!(tuple1(&mut clt, (1,)).await?, (1,));
    Ok(())
}
