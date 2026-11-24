use crate::server::exports::test::resource_borrow_in_record::to_test::{
    Foo, Handler, HandlerThing, Thing,
};

#[derive(Clone)]
pub struct Component(pub crate::Client);

fn contents_of(repr: &[u8]) -> String {
    String::from_utf8(repr.to_vec()).unwrap()
}

fn handle_from(contents: &str) -> ::wit_bindgen_wrpc::wrpc_core::ResourceOwn<Thing> {
    ::wit_bindgen_wrpc::wrpc_core::ResourceOwn::from(
        ::wit_bindgen_wrpc::bytes::Bytes::copy_from_slice(contents.as_bytes()),
    )
}

impl Handler for Component {
    async fn test(
        &self,
        a: Vec<Foo>,
    ) -> ::std::io::Result<
        Vec<::wit_bindgen_wrpc::wrpc_core::ResourceOwn<Thing>>,
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

impl HandlerThing for Component {
    async fn new(
        &self,
        s: String,
    ) -> ::std::io::Result<
        ::wit_bindgen_wrpc::wrpc_core::ResourceOwn<Thing>,
    > {
        Ok(handle_from(&format!("{s} new")))
    }

    async fn get(
        &self,
        self_: ::wit_bindgen_wrpc::wrpc_core::ResourceBorrow<Thing>,
    ) -> ::std::io::Result<String> {
        let contents = contents_of(self_.as_ref());
        Ok(format!("{contents} get"))
    }
}
