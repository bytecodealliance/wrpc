use crate::client::test::records::to_test::*;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    assert_eq!(multiple_results(wrpc, ()).await?, (4, 5));

    assert_eq!(swap_tuple(wrpc, (), (1u8, 2u32)).await?, (2u32, 1u8));
    assert_eq!(roundtrip_flags1(wrpc, (), &F1::A).await?, F1::A);
    assert_eq!(roundtrip_flags1(wrpc, (), &F1::empty()).await?, F1::empty());
    assert_eq!(roundtrip_flags1(wrpc, (), &F1::B).await?, F1::B);
    assert_eq!(
        roundtrip_flags1(wrpc, (), &(F1::A | F1::B)).await?,
        F1::A | F1::B
    );

    assert_eq!(roundtrip_flags2(wrpc, (), &F2::C).await?, F2::C);
    assert_eq!(roundtrip_flags2(wrpc, (), &F2::empty()).await?, F2::empty());
    assert_eq!(roundtrip_flags2(wrpc, (), &F2::D).await?, F2::D);
    assert_eq!(
        roundtrip_flags2(wrpc, (), &(F2::C | F2::E)).await?,
        F2::C | F2::E
    );

    assert_eq!(
        roundtrip_flags3(wrpc, (), &Flag8::B0, &Flag16::B1, &Flag32::B2).await?,
        (Flag8::B0, Flag16::B1, Flag32::B2)
    );

    let r = roundtrip_record1(
        wrpc,
        (),
        &R1 {
            a: 8,
            b: F1::empty(),
        },
    )
    .await?;
    assert_eq!(r.a, 8);
    assert_eq!(r.b, F1::empty());

    let r = roundtrip_record1(
        wrpc,
        (),
        &R1 {
            a: 0,
            b: F1::A | F1::B,
        },
    )
    .await?;
    assert_eq!(r.a, 0);
    assert_eq!(r.b, F1::A | F1::B);

    assert_eq!(tuple1(wrpc, (), (1,)).await?, (1,));
    Ok(())
}
