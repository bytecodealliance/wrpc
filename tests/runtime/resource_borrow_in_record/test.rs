use crate::server::exports::test::resource_borrow_in_record::to_test::{
    Foo, Handler, HandlerThing, Thing,
};

#[derive(Clone)]
pub struct Component;

fn contents_of(repr: &[u8]) -> String {
    String::from_utf8(repr.to_vec()).unwrap()
}

fn handle_from(contents: &str) -> ::wit_bindgen_wrpc::wrpc_transport::ResourceOwn<Thing> {
    ::wit_bindgen_wrpc::wrpc_transport::ResourceOwn::from(
        ::wit_bindgen_wrpc::bytes::Bytes::copy_from_slice(contents.as_bytes()),
    )
}

impl<Ctx: Send> Handler<Ctx> for Component {
    async fn test(
        &self,
        _cx: Ctx,
        a: Vec<Foo>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<
        Vec<::wit_bindgen_wrpc::wrpc_transport::ResourceOwn<Thing>>,
    > {
        Ok(a
            .iter()
            .map(|foo| {
                let contents = contents_of(foo.thing.as_ref());
                handle_from(&format!("{contents} test"))
            })
            .collect())
    }
}

impl<Ctx: Send> HandlerThing<Ctx> for Component {
    async fn new(
        &self,
        _cx: Ctx,
        s: String,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<
        ::wit_bindgen_wrpc::wrpc_transport::ResourceOwn<Thing>,
    > {
        Ok(handle_from(&format!("{s} new")))
    }

    async fn get(
        &self,
        _cx: Ctx,
        self_: ::wit_bindgen_wrpc::wrpc_transport::ResourceBorrow<Thing>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<String> {
        let contents = contents_of(self_.as_ref());
        Ok(format!("{contents} get"))
    }
}
