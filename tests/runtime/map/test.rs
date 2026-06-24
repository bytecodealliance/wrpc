//@ wasmtime-flags = '-Wcomponent-model-map'

use std::collections::HashMap;

use crate::server::exports::test::maps::to_test::{
    BytesByName, Handler, IdsByName, LabeledEntry, MapOrString, NamesById,
};

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> Handler<Ctx> for Component {
    async fn named_roundtrip(
        &self,
        _cx: Ctx,
        a: NamesById,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<IdsByName> {
        assert_eq!(a.get(&1).map(String::as_str), Some("uno"));
        assert_eq!(a.get(&2).map(String::as_str), Some("two"));

        let mut result = IdsByName::new();
        for (id, name) in a {
            result.insert(name, id);
        }
        Ok(result)
    }

    async fn bytes_roundtrip(
        &self,
        _cx: Ctx,
        a: BytesByName,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<BytesByName> {
        assert_eq!(a.get("hello").map(Vec::as_slice), Some(b"world".as_slice()));
        assert_eq!(a.get("bin").map(Vec::as_slice), Some([0u8, 1, 2].as_slice()));
        Ok(a)
    }

    async fn empty_roundtrip(
        &self,
        _cx: Ctx,
        a: NamesById,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<NamesById> {
        assert!(a.is_empty());
        Ok(a)
    }

    async fn option_roundtrip(
        &self,
        _cx: Ctx,
        a: HashMap<String, Option<u32>>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<HashMap<String, Option<u32>>> {
        assert_eq!(a.get("some"), Some(&Some(42)));
        assert_eq!(a.get("none"), Some(&None));
        Ok(a)
    }

    async fn record_roundtrip(
        &self,
        _cx: Ctx,
        a: LabeledEntry,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<LabeledEntry> {
        assert_eq!(a.label, "test-label");
        assert_eq!(a.values.len(), 2);
        assert_eq!(a.values.get(&10).map(String::as_str), Some("ten"));
        assert_eq!(a.values.get(&20).map(String::as_str), Some("twenty"));
        Ok(a)
    }

    async fn inline_roundtrip(
        &self,
        _cx: Ctx,
        a: HashMap<u32, String>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<HashMap<String, u32>> {
        let mut result = HashMap::new();
        for (k, v) in a {
            result.insert(v, k);
        }
        Ok(result)
    }

    async fn large_roundtrip(
        &self,
        _cx: Ctx,
        a: NamesById,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<NamesById> {
        Ok(a)
    }

    async fn multi_param_roundtrip(
        &self,
        _cx: Ctx,
        a: NamesById,
        b: BytesByName,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<(IdsByName, BytesByName)> {
        assert_eq!(a.len(), 2);
        assert_eq!(b.len(), 1);
        let mut ids = IdsByName::new();
        for (id, name) in a {
            ids.insert(name, id);
        }
        Ok((ids, b))
    }

    async fn nested_roundtrip(
        &self,
        _cx: Ctx,
        a: HashMap<String, HashMap<u32, String>>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<HashMap<String, HashMap<u32, String>>> {
        assert_eq!(a.len(), 2);
        let inner = a.get("group-a").unwrap();
        assert_eq!(inner.get(&1).map(String::as_str), Some("one"));
        assert_eq!(inner.get(&2).map(String::as_str), Some("two"));
        let inner2 = a.get("group-b").unwrap();
        assert_eq!(inner2.get(&10).map(String::as_str), Some("ten"));
        Ok(a)
    }

    async fn variant_roundtrip(
        &self,
        _cx: Ctx,
        a: MapOrString,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<MapOrString> {
        Ok(a)
    }

    async fn result_roundtrip(
        &self,
        _cx: Ctx,
        a: Result<NamesById, String>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Result<NamesById, String>> {
        Ok(a)
    }

    async fn tuple_roundtrip(
        &self,
        _cx: Ctx,
        a: (NamesById, u64),
    ) -> ::wit_bindgen_wrpc::anyhow::Result<(NamesById, u64)> {
        assert_eq!(a.0.len(), 1);
        assert_eq!(a.0.get(&7).map(String::as_str), Some("seven"));
        assert_eq!(a.1, 42);
        Ok(a)
    }

    async fn single_entry_roundtrip(
        &self,
        _cx: Ctx,
        a: NamesById,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<NamesById> {
        assert_eq!(a.len(), 1);
        Ok(a)
    }
}
