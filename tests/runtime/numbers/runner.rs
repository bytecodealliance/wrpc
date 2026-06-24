use crate::client::test::numbers::numbers::*;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    assert_eq!(roundtrip_u8(wrpc, (), 1).await?, 1);
    assert_eq!(roundtrip_u8(wrpc, (), u8::MIN).await?, u8::MIN);
    assert_eq!(roundtrip_u8(wrpc, (), u8::MAX).await?, u8::MAX);

    assert_eq!(roundtrip_s8(wrpc, (), 1).await?, 1);
    assert_eq!(roundtrip_s8(wrpc, (), i8::MIN).await?, i8::MIN);
    assert_eq!(roundtrip_s8(wrpc, (), i8::MAX).await?, i8::MAX);

    assert_eq!(roundtrip_u16(wrpc, (), 1).await?, 1);
    assert_eq!(roundtrip_u16(wrpc, (), u16::MIN).await?, u16::MIN);
    assert_eq!(roundtrip_u16(wrpc, (), u16::MAX).await?, u16::MAX);

    assert_eq!(roundtrip_s16(wrpc, (), 1).await?, 1);
    assert_eq!(roundtrip_s16(wrpc, (), i16::MIN).await?, i16::MIN);
    assert_eq!(roundtrip_s16(wrpc, (), i16::MAX).await?, i16::MAX);

    assert_eq!(roundtrip_u32(wrpc, (), 1).await?, 1);
    assert_eq!(roundtrip_u32(wrpc, (), u32::MIN).await?, u32::MIN);
    assert_eq!(roundtrip_u32(wrpc, (), u32::MAX).await?, u32::MAX);

    assert_eq!(roundtrip_s32(wrpc, (), 1).await?, 1);
    assert_eq!(roundtrip_s32(wrpc, (), i32::MIN).await?, i32::MIN);
    assert_eq!(roundtrip_s32(wrpc, (), i32::MAX).await?, i32::MAX);

    assert_eq!(roundtrip_u64(wrpc, (), 1).await?, 1);
    assert_eq!(roundtrip_u64(wrpc, (), u64::MIN).await?, u64::MIN);
    assert_eq!(roundtrip_u64(wrpc, (), u64::MAX).await?, u64::MAX);

    assert_eq!(roundtrip_s64(wrpc, (), 1).await?, 1);
    assert_eq!(roundtrip_s64(wrpc, (), i64::MIN).await?, i64::MIN);
    assert_eq!(roundtrip_s64(wrpc, (), i64::MAX).await?, i64::MAX);

    assert_eq!(roundtrip_f32(wrpc, (), 1.0).await?, 1.0);
    assert_eq!(roundtrip_f32(wrpc, (), f32::INFINITY).await?, f32::INFINITY);
    assert_eq!(
        roundtrip_f32(wrpc, (), f32::NEG_INFINITY).await?,
        f32::NEG_INFINITY
    );
    assert!(roundtrip_f32(wrpc, (), f32::NAN).await?.is_nan());

    assert_eq!(roundtrip_f64(wrpc, (), 1.0).await?, 1.0);
    assert_eq!(roundtrip_f64(wrpc, (), f64::INFINITY).await?, f64::INFINITY);
    assert_eq!(
        roundtrip_f64(wrpc, (), f64::NEG_INFINITY).await?,
        f64::NEG_INFINITY
    );
    assert!(roundtrip_f64(wrpc, (), f64::NAN).await?.is_nan());

    assert_eq!(roundtrip_char(wrpc, (), 'a').await?, 'a');
    assert_eq!(roundtrip_char(wrpc, (), ' ').await?, ' ');
    assert_eq!(roundtrip_char(wrpc, (), '🚩').await?, '🚩');

    set_scalar(wrpc, (), 2).await?;
    assert_eq!(get_scalar(wrpc, ()).await?, 2);
    set_scalar(wrpc, (), 4).await?;
    assert_eq!(get_scalar(wrpc, ()).await?, 4);
    Ok(())
}
