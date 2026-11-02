use crate::leaf::exports::my::inline::foo::{A, Handler};

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> Handler<Ctx> for Component {
    async fn bar(&self, _cx: Ctx, a: A) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        assert_eq!(a.b, 2);
        Ok(())
    }
}
