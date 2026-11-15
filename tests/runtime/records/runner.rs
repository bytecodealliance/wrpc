use crate::client::test::records::to_test::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(multiple_results(tx, rx).await?, (4, 5));

    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(swap_tuple(tx, rx, (1u8, 2u32)).await?, (2u32, 1u8));
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_flags1(tx, rx, &F1::A).await?, F1::A);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_flags1(tx, rx, &F1::empty()).await?, F1::empty());
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_flags1(tx, rx, &F1::B).await?, F1::B);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        roundtrip_flags1(tx, rx, &(F1::A | F1::B)).await?,
        F1::A | F1::B
    );

    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_flags2(tx, rx, &F2::C).await?, F2::C);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_flags2(tx, rx, &F2::empty()).await?, F2::empty());
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_flags2(tx, rx, &F2::D).await?, F2::D);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        roundtrip_flags2(tx, rx, &(F2::C | F2::E)).await?,
        F2::C | F2::E
    );

    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        roundtrip_flags3(tx, rx, &Flag8::B0, &Flag16::B1, &Flag32::B2).await?,
        (Flag8::B0, Flag16::B1, Flag32::B2)
    );

    let (tx, rx) = wrpc.connect().await?;
    let r = roundtrip_record1(
        tx,
        rx,
        &R1 {
            a: 8,
            b: F1::empty(),
        },
    )
    .await?;
    assert_eq!(r.a, 8);
    assert_eq!(r.b, F1::empty());

    let (tx, rx) = wrpc.connect().await?;
    let r = roundtrip_record1(
        tx,
        rx,
        &R1 {
            a: 0,
            b: F1::A | F1::B,
        },
    )
    .await?;
    assert_eq!(r.a, 0);
    assert_eq!(r.b, F1::A | F1::B);

    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(tuple1(tx, rx, (1,)).await?, (1,));
    Ok(())
}
