use ::wit_bindgen_wrpc::bytes::Bytes;

use crate::client::test::lists::to_test::*;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    empty_list_param(wrpc, (), &Bytes::new()).await?;
    empty_string_param(wrpc, (), "").await?;
    assert!(empty_list_result(wrpc, ()).await?.is_empty());
    assert!(empty_string_result(wrpc, ()).await?.is_empty());

    list_param(wrpc, (), &Bytes::from_static(&[1, 2, 3, 4])).await?;
    list_param2(wrpc, (), "foo").await?;
    list_param3(wrpc, (), &["foo", "bar", "baz"]).await?;
    list_param4(wrpc, (), &[&["foo", "bar"][..], &["baz"][..]]).await?;
    list_param5(wrpc, (), &[(1, 2, 3), (4, 5, 6)]).await?;
    let large_list: Vec<String> = (0..1000).map(|_| "string".to_string()).collect();
    let large_list_refs: Vec<&str> = large_list.iter().map(String::as_str).collect();
    list_param_large(wrpc, (), &large_list_refs).await?;
    assert_eq!(list_result(wrpc, ()).await?, &[1, 2, 3, 4, 5][..]);
    assert_eq!(list_result2(wrpc, ()).await?, "hello!");
    assert_eq!(list_result3(wrpc, ()).await?, ["hello,", "world!"]);

    assert_eq!(list_roundtrip(wrpc, (), &Bytes::new()).await?, &[][..]);
    assert_eq!(
        list_roundtrip(wrpc, (), &Bytes::from_static(b"x")).await?,
        &b"x"[..]
    );
    assert_eq!(
        list_roundtrip(wrpc, (), &Bytes::from_static(b"hello")).await?,
        &b"hello"[..]
    );

    assert_eq!(string_roundtrip(wrpc, (), "x").await?, "x");
    assert_eq!(string_roundtrip(wrpc, (), "").await?, "");
    assert_eq!(string_roundtrip(wrpc, (), "hello").await?, "hello");
    assert_eq!(
        string_roundtrip(wrpc, (), "hello ⚑ world").await?,
        "hello ⚑ world"
    );

    assert_eq!(
        list_minmax8(
            wrpc,
            (),
            &Bytes::from_static(&[u8::MIN, u8::MAX]),
            &[i8::MIN, i8::MAX]
        )
        .await?,
        (
            Bytes::from_static(&[u8::MIN, u8::MAX]),
            vec![i8::MIN, i8::MAX]
        ),
    );
    assert_eq!(
        list_minmax16(wrpc, (), &[u16::MIN, u16::MAX], &[i16::MIN, i16::MAX]).await?,
        (vec![u16::MIN, u16::MAX], vec![i16::MIN, i16::MAX]),
    );
    assert_eq!(
        list_minmax32(wrpc, (), &[u32::MIN, u32::MAX], &[i32::MIN, i32::MAX]).await?,
        (vec![u32::MIN, u32::MAX], vec![i32::MIN, i32::MAX]),
    );
    assert_eq!(
        list_minmax64(wrpc, (), &[u64::MIN, u64::MAX], &[i64::MIN, i64::MAX]).await?,
        (vec![u64::MIN, u64::MAX], vec![i64::MIN, i64::MAX]),
    );
    assert_eq!(
        list_minmax_float(
            wrpc,
            (),
            &[f32::MIN, f32::MAX, f32::NEG_INFINITY, f32::INFINITY],
            &[f64::MIN, f64::MAX, f64::NEG_INFINITY, f64::INFINITY]
        )
        .await?,
        (
            vec![f32::MIN, f32::MAX, f32::NEG_INFINITY, f32::INFINITY],
            vec![f64::MIN, f64::MAX, f64::NEG_INFINITY, f64::INFINITY],
        ),
    );

    let headers = [
        ("Content-Type", Bytes::from_static(b"text/plain")),
        (
            "Content-Length",
            Bytes::from("Not found".len().to_string().into_bytes()),
        ),
    ];
    let headers_refs: Vec<(&str, &Bytes)> = headers.iter().map(|(k, v)| (*k, v)).collect();
    let result = wasi_http_headers_roundtrip(wrpc, (), &headers_refs).await?;
    assert_eq!(result[0].0, "Content-Type");
    assert_eq!(result[0].1, &b"text/plain"[..]);
    assert_eq!(result[1].0, "Content-Length");
    assert_eq!(
        result[1].1,
        Bytes::from("Not found".len().to_string().into_bytes())
    );
    Ok(())
}
