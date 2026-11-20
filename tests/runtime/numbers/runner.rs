use crate::client::test::numbers::numbers::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let mut clt = wrpc.connect().await?;
    assert_eq!(roundtrip_u8(&mut clt, 1).await?, 1);
    assert_eq!(roundtrip_u8(&mut clt, u8::MIN).await?, u8::MIN);
    assert_eq!(roundtrip_u8(&mut clt, u8::MAX).await?, u8::MAX);

    assert_eq!(roundtrip_s8(&mut clt, 1).await?, 1);
    assert_eq!(roundtrip_s8(&mut clt, i8::MIN).await?, i8::MIN);
    assert_eq!(roundtrip_s8(&mut clt, i8::MAX).await?, i8::MAX);

    assert_eq!(roundtrip_u16(&mut clt, 1).await?, 1);
    assert_eq!(roundtrip_u16(&mut clt, u16::MIN).await?, u16::MIN);
    assert_eq!(roundtrip_u16(&mut clt, u16::MAX).await?, u16::MAX);

    assert_eq!(roundtrip_s16(&mut clt, 1).await?, 1);
    assert_eq!(roundtrip_s16(&mut clt, i16::MIN).await?, i16::MIN);
    assert_eq!(roundtrip_s16(&mut clt, i16::MAX).await?, i16::MAX);

    assert_eq!(roundtrip_u32(&mut clt, 1).await?, 1);
    assert_eq!(roundtrip_u32(&mut clt, u32::MIN).await?, u32::MIN);
    assert_eq!(roundtrip_u32(&mut clt, u32::MAX).await?, u32::MAX);

    assert_eq!(roundtrip_s32(&mut clt, 1).await?, 1);
    assert_eq!(roundtrip_s32(&mut clt, i32::MIN).await?, i32::MIN);
    assert_eq!(roundtrip_s32(&mut clt, i32::MAX).await?, i32::MAX);

    assert_eq!(roundtrip_u64(&mut clt, 1).await?, 1);
    assert_eq!(roundtrip_u64(&mut clt, u64::MIN).await?, u64::MIN);
    assert_eq!(roundtrip_u64(&mut clt, u64::MAX).await?, u64::MAX);

    assert_eq!(roundtrip_s64(&mut clt, 1).await?, 1);
    assert_eq!(roundtrip_s64(&mut clt, i64::MIN).await?, i64::MIN);
    assert_eq!(roundtrip_s64(&mut clt, i64::MAX).await?, i64::MAX);

    assert_eq!(roundtrip_f32(&mut clt, 1.0).await?, 1.0);
    assert_eq!(roundtrip_f32(&mut clt, f32::INFINITY).await?, f32::INFINITY);
    assert_eq!(
        roundtrip_f32(&mut clt, f32::NEG_INFINITY).await?,
        f32::NEG_INFINITY
    );
    assert!(roundtrip_f32(&mut clt, f32::NAN).await?.is_nan());

    assert_eq!(roundtrip_f64(&mut clt, 1.0).await?, 1.0);
    assert_eq!(roundtrip_f64(&mut clt, f64::INFINITY).await?, f64::INFINITY);
    assert_eq!(
        roundtrip_f64(&mut clt, f64::NEG_INFINITY).await?,
        f64::NEG_INFINITY
    );
    assert!(roundtrip_f64(&mut clt, f64::NAN).await?.is_nan());

    assert_eq!(roundtrip_char(&mut clt, 'a').await?, 'a');
    assert_eq!(roundtrip_char(&mut clt, ' ').await?, ' ');
    assert_eq!(roundtrip_char(&mut clt, '🚩').await?, '🚩');

    set_scalar(&mut clt, 2).await?;
    assert_eq!(get_scalar(&mut clt).await?, 2);
    set_scalar(&mut clt, 4).await?;
    assert_eq!(get_scalar(&mut clt).await?, 4);
    Ok(())
}
