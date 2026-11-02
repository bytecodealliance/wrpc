use crate::server::exports::a::b::i::Handler;

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> Handler<Ctx> for Component {
    async fn one_argument(&self, _cx: Ctx, x: u32) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        assert_eq!(x, 1);
        Ok(())
    }

    async fn one_result(&self, _cx: Ctx) -> ::wit_bindgen_wrpc::anyhow::Result<u32> {
        Ok(2)
    }

    async fn one_argument_and_result(
        &self,
        _cx: Ctx,
        x: u32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<u32> {
        assert_eq!(x, 3);
        Ok(4)
    }

    async fn two_arguments(
        &self,
        _cx: Ctx,
        x: u32,
        y: u32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        assert_eq!(x, 5);
        assert_eq!(y, 6);
        Ok(())
    }

    async fn two_arguments_and_result(
        &self,
        _cx: Ctx,
        x: u32,
        y: u32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<u32> {
        assert_eq!(x, 7);
        assert_eq!(y, 8);
        Ok(9)
    }
}
