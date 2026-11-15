use crate::client::test::numbers::numbers::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_u8(tx, rx, 1).await?, 1);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_u8(tx, rx, u8::MIN).await?, u8::MIN);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_u8(tx, rx, u8::MAX).await?, u8::MAX);

    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_s8(tx, rx, 1).await?, 1);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_s8(tx, rx, i8::MIN).await?, i8::MIN);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_s8(tx, rx, i8::MAX).await?, i8::MAX);

    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_u16(tx, rx, 1).await?, 1);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_u16(tx, rx, u16::MIN).await?, u16::MIN);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_u16(tx, rx, u16::MAX).await?, u16::MAX);

    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_s16(tx, rx, 1).await?, 1);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_s16(tx, rx, i16::MIN).await?, i16::MIN);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_s16(tx, rx, i16::MAX).await?, i16::MAX);

    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_u32(tx, rx, 1).await?, 1);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_u32(tx, rx, u32::MIN).await?, u32::MIN);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_u32(tx, rx, u32::MAX).await?, u32::MAX);

    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_s32(tx, rx, 1).await?, 1);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_s32(tx, rx, i32::MIN).await?, i32::MIN);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_s32(tx, rx, i32::MAX).await?, i32::MAX);

    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_u64(tx, rx, 1).await?, 1);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_u64(tx, rx, u64::MIN).await?, u64::MIN);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_u64(tx, rx, u64::MAX).await?, u64::MAX);

    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_s64(tx, rx, 1).await?, 1);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_s64(tx, rx, i64::MIN).await?, i64::MIN);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_s64(tx, rx, i64::MAX).await?, i64::MAX);

    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_f32(tx, rx, 1.0).await?, 1.0);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_f32(tx, rx, f32::INFINITY).await?, f32::INFINITY);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        roundtrip_f32(tx, rx, f32::NEG_INFINITY).await?,
        f32::NEG_INFINITY
    );
    let (tx, rx) = wrpc.connect().await?;
    assert!(roundtrip_f32(tx, rx, f32::NAN).await?.is_nan());

    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_f64(tx, rx, 1.0).await?, 1.0);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_f64(tx, rx, f64::INFINITY).await?, f64::INFINITY);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        roundtrip_f64(tx, rx, f64::NEG_INFINITY).await?,
        f64::NEG_INFINITY
    );
    let (tx, rx) = wrpc.connect().await?;
    assert!(roundtrip_f64(tx, rx, f64::NAN).await?.is_nan());

    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_char(tx, rx, 'a').await?, 'a');
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_char(tx, rx, ' ').await?, ' ');
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(roundtrip_char(tx, rx, '🚩').await?, '🚩');

    let (tx, rx) = wrpc.connect().await?;
    set_scalar(tx, rx, 2).await?;
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(get_scalar(tx, rx).await?, 2);
    let (tx, rx) = wrpc.connect().await?;
    set_scalar(tx, rx, 4).await?;
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(get_scalar(tx, rx).await?, 4);
    Ok(())
}
