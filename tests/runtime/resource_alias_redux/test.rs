use crate::test::exports::test::resource_alias_redux::resource_alias1 as a1;
use crate::test::exports::test::resource_alias_redux::resource_alias2 as a2;
use crate::test::exports::the_test;

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> a1::Handler<Ctx> for Component {
    async fn a(
        &self,
        _cx: Ctx,
        f: a1::Foo,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<
        Vec<::wit_bindgen_wrpc::wrpc_transport::ResourceOwn<a1::Thing>>,
    > {
        Ok(vec![f.thing])
    }
}

impl<Ctx: Send> a1::HandlerThing<Ctx> for Component {
    async fn new(
        &self,
        _cx: Ctx,
        s: String,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<
        ::wit_bindgen_wrpc::wrpc_transport::ResourceOwn<a1::Thing>,
    > {
        let contents = format!("{s} GuestThing");
        Ok(::wit_bindgen_wrpc::wrpc_transport::ResourceOwn::from(
            ::wit_bindgen_wrpc::bytes::Bytes::copy_from_slice(contents.as_bytes()),
        ))
    }

    async fn get(
        &self,
        _cx: Ctx,
        self_: ::wit_bindgen_wrpc::wrpc_transport::ResourceBorrow<a1::Thing>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<String> {
        let repr: &[u8] = self_.as_ref();
        let contents = String::from_utf8(repr.to_vec()).unwrap();
        Ok(format!("{contents} GuestThing.get"))
    }
}

impl<Ctx: Send> a2::Handler<Ctx> for Component {
    async fn b(
        &self,
        _cx: Ctx,
        f: a2::Foo,
        g: a2::Bar,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<
        Vec<::wit_bindgen_wrpc::wrpc_transport::ResourceOwn<a2::Thing>>,
    > {
        Ok(vec![f.thing, g.thing])
    }
}

impl<Ctx: Send> the_test::Handler<Ctx> for Component {
    async fn test(
        &self,
        _cx: Ctx,
        things: Vec<::wit_bindgen_wrpc::wrpc_transport::ResourceOwn<the_test::Thing>>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<
        Vec<::wit_bindgen_wrpc::wrpc_transport::ResourceOwn<the_test::Thing>>,
    > {
        Ok(things)
    }
}
