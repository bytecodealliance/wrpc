//@ wasmtime-flags = '-Wcomponent-model-map'

use std::collections::HashMap;

use crate::client::test::maps::to_test::*;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    test_named_roundtrip(wrpc).await?;
    test_bytes_roundtrip(wrpc).await?;
    test_empty_roundtrip(wrpc).await?;
    test_option_roundtrip(wrpc).await?;
    test_record_roundtrip(wrpc).await?;
    test_inline_roundtrip(wrpc).await?;
    test_large_map(wrpc).await?;
    test_multi_param_roundtrip(wrpc).await?;
    test_nested_roundtrip(wrpc).await?;
    test_variant_roundtrip(wrpc).await?;
    test_result_roundtrip(wrpc).await?;
    test_tuple_roundtrip(wrpc).await?;
    test_single_entry_roundtrip(wrpc).await?;
    Ok(())
}

async fn test_named_roundtrip(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let mut input = NamesById::new();
    input.insert(1, "one".to_string());
    input.insert(1, "uno".to_string());
    input.insert(2, "two".to_string());
    let ids_by_name = named_roundtrip(wrpc, (), &input).await?;
    assert_eq!(ids_by_name.get("uno"), Some(&1));
    assert_eq!(ids_by_name.get("two"), Some(&2));
    assert_eq!(ids_by_name.get("one"), None);
    Ok(())
}

async fn test_bytes_roundtrip(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let mut bytes_input = BytesByName::new();
    bytes_input.insert("hello".to_string(), b"world".to_vec());
    bytes_input.insert("bin".to_string(), vec![0u8, 1, 2]);
    let bytes_by_name = bytes_roundtrip(wrpc, (), &bytes_input).await?;
    assert_eq!(
        bytes_by_name.get("hello").map(Vec::as_slice),
        Some(b"world".as_slice())
    );
    assert_eq!(
        bytes_by_name.get("bin").map(Vec::as_slice),
        Some([0u8, 1, 2].as_slice())
    );
    Ok(())
}

async fn test_empty_roundtrip(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let empty = NamesById::new();
    let result = empty_roundtrip(wrpc, (), &empty).await?;
    assert!(result.is_empty());
    Ok(())
}

async fn test_option_roundtrip(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let mut input = HashMap::new();
    input.insert("some".to_string(), Some(42));
    input.insert("none".to_string(), None);
    let result = option_roundtrip(wrpc, (), &input).await?;
    assert_eq!(result.len(), 2);
    assert_eq!(result.get("some"), Some(&Some(42)));
    assert_eq!(result.get("none"), Some(&None));
    Ok(())
}

async fn test_record_roundtrip(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let mut values = NamesById::new();
    values.insert(10, "ten".to_string());
    values.insert(20, "twenty".to_string());
    let entry = LabeledEntry {
        label: "test-label".to_string(),
        values,
    };
    let result = record_roundtrip(wrpc, (), &entry).await?;
    assert_eq!(result.label, "test-label");
    assert_eq!(result.values.len(), 2);
    assert_eq!(result.values.get(&10).map(String::as_str), Some("ten"));
    assert_eq!(result.values.get(&20).map(String::as_str), Some("twenty"));
    Ok(())
}

async fn test_inline_roundtrip(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let mut input = HashMap::new();
    input.insert(1, "one".to_string());
    input.insert(2, "two".to_string());
    let result = inline_roundtrip(wrpc, (), &input).await?;
    assert_eq!(result.len(), 2);
    assert_eq!(result.get("one"), Some(&1));
    assert_eq!(result.get("two"), Some(&2));
    Ok(())
}

async fn test_large_map(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let mut input = NamesById::new();
    for i in 0..100 {
        input.insert(i, format!("value-{i}"));
    }
    let result = large_roundtrip(wrpc, (), &input).await?;
    assert_eq!(result.len(), 100);
    for i in 0..100 {
        assert_eq!(
            result.get(&i).map(String::as_str),
            Some(format!("value-{i}").as_str()),
        );
    }
    Ok(())
}

async fn test_multi_param_roundtrip(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let mut names = NamesById::new();
    names.insert(1, "one".to_string());
    names.insert(2, "two".to_string());
    let mut bytes = BytesByName::new();
    bytes.insert("key".to_string(), vec![42u8]);
    let (ids, bytes_out) = multi_param_roundtrip(wrpc, (), &names, &bytes).await?;
    assert_eq!(ids.len(), 2);
    assert_eq!(ids.get("one"), Some(&1));
    assert_eq!(ids.get("two"), Some(&2));
    assert_eq!(bytes_out.len(), 1);
    assert_eq!(
        bytes_out.get("key").map(Vec::as_slice),
        Some([42u8].as_slice()),
    );
    Ok(())
}

async fn test_nested_roundtrip(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let mut inner_a = HashMap::new();
    inner_a.insert(1, "one".to_string());
    inner_a.insert(2, "two".to_string());
    let mut inner_b = HashMap::new();
    inner_b.insert(10, "ten".to_string());
    let mut outer = HashMap::new();
    outer.insert("group-a".to_string(), inner_a);
    outer.insert("group-b".to_string(), inner_b);
    let result = nested_roundtrip(wrpc, (), &outer).await?;
    assert_eq!(result.len(), 2);
    let ra = result.get("group-a").unwrap();
    assert_eq!(ra.get(&1).map(String::as_str), Some("one"));
    assert_eq!(ra.get(&2).map(String::as_str), Some("two"));
    let rb = result.get("group-b").unwrap();
    assert_eq!(rb.get(&10).map(String::as_str), Some("ten"));
    Ok(())
}

async fn test_variant_roundtrip(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let mut map = NamesById::new();
    map.insert(1, "one".to_string());
    let as_map = variant_roundtrip(wrpc, (), &MapOrString::AsMap(map)).await?;
    match &as_map {
        MapOrString::AsMap(m) => {
            assert_eq!(m.get(&1).map(String::as_str), Some("one"));
        }
        MapOrString::AsString(_) => panic!("expected AsMap"),
    }

    let as_str = variant_roundtrip(wrpc, (), &MapOrString::AsString("hello".to_string())).await?;
    match &as_str {
        MapOrString::AsString(s) => assert_eq!(s, "hello"),
        MapOrString::AsMap(_) => panic!("expected AsString"),
    }
    Ok(())
}

async fn test_result_roundtrip(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let mut map = NamesById::new();
    map.insert(5, "five".to_string());
    let ok_result = result_roundtrip(wrpc, (), Ok(&map)).await?;
    match &ok_result {
        Ok(m) => assert_eq!(m.get(&5).map(String::as_str), Some("five")),
        Err(_) => panic!("expected Ok"),
    }

    let err_result = result_roundtrip(wrpc, (), Err("bad input")).await?;
    match &err_result {
        Err(e) => assert_eq!(e, "bad input"),
        Ok(_) => panic!("expected Err"),
    }
    Ok(())
}

async fn test_tuple_roundtrip(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let mut map = NamesById::new();
    map.insert(7, "seven".to_string());
    let (result_map, result_num) = tuple_roundtrip(wrpc, (), (&map, 42)).await?;
    assert_eq!(result_map.len(), 1);
    assert_eq!(result_map.get(&7).map(String::as_str), Some("seven"));
    assert_eq!(result_num, 42);
    Ok(())
}

async fn test_single_entry_roundtrip(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let mut input = NamesById::new();
    input.insert(99, "ninety-nine".to_string());
    let result = single_entry_roundtrip(wrpc, (), &input).await?;
    assert_eq!(result.len(), 1);
    assert_eq!(result.get(&99).map(String::as_str), Some("ninety-nine"));
    Ok(())
}
