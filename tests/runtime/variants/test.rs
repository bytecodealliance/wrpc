use crate::server::exports::test::variants::to_test::*;

#[derive(Clone)]
pub struct Component(pub crate::Client);

impl crate::server::exports::test::variants::to_test::Handler for Component {
    async fn roundtrip_option(
        &self,
        a: Option<f32>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Option<u8>> {
        Ok(a.map(|x| x as u8))
    }

    async fn roundtrip_result(
        &self,
        a: Result<u32, f32>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Result<f64, u8>> {
        Ok(match a {
            Ok(a) => Ok(a.into()),
            Err(b) => Err(b as u8),
        })
    }

    async fn roundtrip_enum(&self, a: E1) -> ::wit_bindgen_wrpc::anyhow::Result<E1> {
        assert_eq!(a, a);
        Ok(a)
    }

    async fn invert_bool(&self, a: bool) -> ::wit_bindgen_wrpc::anyhow::Result<bool> {
        Ok(!a)
    }

    async fn variant_casts(
        &self,
        a: Casts,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Casts> {
        Ok(a)
    }

    async fn variant_zeros(
        &self,
        a: Zeros,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Zeros> {
        Ok(a)
    }

    async fn variant_typedefs(
        &self,
        _a: Option<u32>,
        _b: bool,
        _c: Result<u32, ()>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        Ok(())
    }

    async fn variant_enums(
        &self,
        a: bool,
        b: Result<(), ()>,
        c: MyErrno,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<(bool, Result<(), ()>, MyErrno)> {
        Ok((a, b, c))
    }
}
