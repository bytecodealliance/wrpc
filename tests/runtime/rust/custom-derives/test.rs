//@ args = [
//@   '-dHash',
//@   '-dClone',
//@   '-d::core::cmp::PartialEq',
//@   '-d::core::cmp::Eq',
//@   '-dserde::Serialize',
//@   '-dserde::Deserialize',
//@   '--additional-derive-ignore=ignoreme',
//@ ]

use std::collections::{hash_map::RandomState, HashSet};

use exports::my::inline::blah::Foo;

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> exports::my::inline::blah::Handler<Ctx> for Component {
    async fn bar(&self, _cx: Ctx, cool: Foo) -> anyhow::Result<()> {
        // The added `Hash`/`Eq` derives must apply to `foo`, so it can be used
        // as a `HashSet` element.
        let _blah: HashSet<Foo, RandomState> = HashSet::from_iter([Foo {
            field1: "hello".to_string(),
            field2: vec![1, 2, 3],
        }]);

        // The added `serde` derives must apply too, otherwise this fails to
        // compile.
        let _ = serde_json::to_string(&cool);
        Ok(())
    }

    async fn barry(
        &self,
        _cx: Ctx,
        warm: exports::my::inline::blah::Ignoreme,
    ) -> anyhow::Result<()> {
        // Compilation would fail here if `serde::Deserialize` were applied to
        // `ignoreme`, since it holds a resource handle. `--additional-derive-ignore`
        // must have excluded it.
        let _ = warm;
        Ok(())
    }
}
