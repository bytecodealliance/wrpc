use ::wit_bindgen_wrpc::bytes::Bytes;

use crate::client::test::lists::to_test::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    empty_list_param(tx, rx, &Bytes::new()).await?;
    let (tx, rx) = wrpc.connect().await?;
    empty_string_param(tx, rx, "").await?;
    let (tx, rx) = wrpc.connect().await?;
    assert!(empty_list_result(tx, rx).await?.is_empty());
    let (tx, rx) = wrpc.connect().await?;
    assert!(empty_string_result(tx, rx).await?.is_empty());

    let (tx, rx) = wrpc.connect().await?;
    list_param(tx, rx, &Bytes::from_static(&[1, 2, 3, 4])).await?;
    let (tx, rx) = wrpc.connect().await?;
    list_param2(tx, rx, "foo").await?;
    let (tx, rx) = wrpc.connect().await?;
    list_param3(tx, rx, &["foo", "bar", "baz"]).await?;
    let (tx, rx) = wrpc.connect().await?;
    list_param4(tx, rx, &[&["foo", "bar"][..], &["baz"][..]]).await?;
    let (tx, rx) = wrpc.connect().await?;
    list_param5(tx, rx, &[(1, 2, 3), (4, 5, 6)]).await?;
    let large_list: Vec<String> = (0..1000).map(|_| "string".to_string()).collect();
    let large_list_refs: Vec<&str> = large_list.iter().map(String::as_str).collect();
    let (tx, rx) = wrpc.connect().await?;
    list_param_large(tx, rx, &large_list_refs).await?;
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(list_result(tx, rx).await?, &[1, 2, 3, 4, 5][..]);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(list_result2(tx, rx).await?, "hello!");
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(list_result3(tx, rx).await?, ["hello,", "world!"]);

    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(list_roundtrip(tx, rx, &Bytes::new()).await?, &[][..]);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        list_roundtrip(tx, rx, &Bytes::from_static(b"x")).await?,
        &b"x"[..]
    );
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        list_roundtrip(tx, rx, &Bytes::from_static(b"hello")).await?,
        &b"hello"[..]
    );

    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(string_roundtrip(tx, rx, "x").await?, "x");
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(string_roundtrip(tx, rx, "").await?, "");
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(string_roundtrip(tx, rx, "hello").await?, "hello");
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        string_roundtrip(tx, rx, "hello ⚑ world").await?,
        "hello ⚑ world"
    );

    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        list_minmax8(
            tx,
            rx,
            &Bytes::from_static(&[u8::MIN, u8::MAX]),
            &[i8::MIN, i8::MAX]
        )
        .await?,
        (
            Bytes::from_static(&[u8::MIN, u8::MAX]),
            vec![i8::MIN, i8::MAX]
        ),
    );
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        list_minmax16(tx, rx, &[u16::MIN, u16::MAX], &[i16::MIN, i16::MAX]).await?,
        (vec![u16::MIN, u16::MAX], vec![i16::MIN, i16::MAX]),
    );
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        list_minmax32(tx, rx, &[u32::MIN, u32::MAX], &[i32::MIN, i32::MAX]).await?,
        (vec![u32::MIN, u32::MAX], vec![i32::MIN, i32::MAX]),
    );
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        list_minmax64(tx, rx, &[u64::MIN, u64::MAX], &[i64::MIN, i64::MAX]).await?,
        (vec![u64::MIN, u64::MAX], vec![i64::MIN, i64::MAX]),
    );
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        list_minmax_float(
            tx,
            rx,
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
    let (tx, rx) = wrpc.connect().await?;
    let result = wasi_http_headers_roundtrip(tx, rx, &headers_refs).await?;
    assert_eq!(result[0].0, "Content-Type");
    assert_eq!(result[0].1, &b"text/plain"[..]);
    assert_eq!(result[1].0, "Content-Length");
    assert_eq!(
        result[1].1,
        Bytes::from("Not found".len().to_string().into_bytes())
    );
    Ok(())
}
