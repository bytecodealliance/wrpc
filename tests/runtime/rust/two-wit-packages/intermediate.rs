use ::wit_bindgen_wrpc::wrpc_transport::Invoke;

#[derive(Clone)]
pub struct Component;

impl<C: Invoke<Context = ()>> crate::server::exports::a::b::x::Handler<C> for Component {
    async fn f1(&self, cx: C) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        crate::server::a::b::x::f1(&cx, ()).await
    }
}

impl<C: Invoke<Context = ()>> crate::server::exports::other::c::x::Handler<C> for Component {
    async fn f2(&self, cx: C) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        crate::server::other::c::x::f2(&cx, ()).await
    }
}
