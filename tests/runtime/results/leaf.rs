use crate::leaf::exports::test::results::test as imports;

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> imports::Handler<Ctx> for Component {
    async fn string_error(
        &self,
        _cx: Ctx,
        a: f32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Result<f32, String>> {
        if a == 0.0 {
            Ok(Err("zero".to_owned()))
        } else {
            Ok(Ok(a))
        }
    }

    async fn enum_error(
        &self,
        _cx: Ctx,
        a: f32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Result<f32, imports::E>> {
        if a == 0.0 {
            Ok(Err(imports::E::A))
        } else {
            Ok(Ok(a))
        }
    }

    async fn record_error(
        &self,
        _cx: Ctx,
        a: f32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Result<f32, imports::E2>> {
        if a == 0.0 {
            Ok(Err(imports::E2 {
                line: 420,
                column: 0,
            }))
        } else if a == 1.0 {
            Ok(Err(imports::E2 {
                line: 77,
                column: 2,
            }))
        } else {
            Ok(Ok(a))
        }
    }

    async fn variant_error(
        &self,
        _cx: Ctx,
        a: f32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Result<f32, imports::E3>> {
        if a == 0.0 {
            Ok(Err(imports::E3::E2(imports::E2 {
                line: 420,
                column: 0,
            })))
        } else if a == 1.0 {
            Ok(Err(imports::E3::E1(imports::E::B)))
        } else if a == 2.0 {
            Ok(Err(imports::E3::E1(imports::E::C)))
        } else {
            Ok(Ok(a))
        }
    }

    async fn empty_error(
        &self,
        _cx: Ctx,
        a: u32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Result<u32, ()>> {
        if a == 0 {
            Ok(Err(()))
        } else if a == 1 {
            Ok(Ok(42))
        } else {
            Ok(Ok(a))
        }
    }

    async fn double_error(
        &self,
        _cx: Ctx,
        a: u32,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Result<Result<(), String>, String>> {
        if a == 0 {
            Ok(Ok(Ok(())))
        } else if a == 1 {
            Ok(Ok(Err("one".into())))
        } else {
            Ok(Err("two".into()))
        }
    }
}
