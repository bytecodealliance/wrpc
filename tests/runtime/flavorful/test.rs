use crate::test::exports::test::flavorful::to_test::*;

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> crate::test::exports::test::flavorful::to_test::Handler<Ctx> for Component {
    async fn f_list_in_record1(
        &self,
        _cx: Ctx,
        ty: ListInRecord1,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        assert_eq!(ty.a, "list_in_record1");
        Ok(())
    }

    async fn f_list_in_record2(
        &self,
        _cx: Ctx,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ListInRecord2> {
        Ok(ListInRecord2 {
            a: "list_in_record2".to_string(),
        })
    }

    async fn f_list_in_record3(
        &self,
        _cx: Ctx,
        a: ListInRecord3,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ListInRecord3> {
        assert_eq!(a.a, "list_in_record3 input");
        Ok(ListInRecord3 {
            a: "list_in_record3 output".to_string(),
        })
    }

    async fn f_list_in_record4(
        &self,
        _cx: Ctx,
        a: ListInAlias,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ListInAlias> {
        assert_eq!(a.a, "input4");
        Ok(ListInRecord4 {
            a: "result4".to_string(),
        })
    }

    async fn f_list_in_variant1(
        &self,
        _cx: Ctx,
        a: ListInVariant1V1,
        b: ListInVariant1V2,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
        assert_eq!(a.unwrap(), "foo");
        assert_eq!(b.unwrap_err(), "bar");
        Ok(())
    }

    async fn f_list_in_variant2(
        &self,
        _cx: Ctx,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ListInVariant2> {
        Ok(Some("list_in_variant2".to_string()))
    }

    async fn f_list_in_variant3(
        &self,
        _cx: Ctx,
        a: ListInVariant3,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<ListInVariant3> {
        assert_eq!(a.unwrap(), "input3");
        Ok(Some("output3".to_string()))
    }

    async fn errno_result(
        &self,
        _cx: Ctx,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<::core::result::Result<(), MyErrno>> {
        static FIRST: ::std::sync::atomic::AtomicBool = ::std::sync::atomic::AtomicBool::new(true);
        MyErrno::A.to_string();
        _ = format!("{:?}", MyErrno::A);
        fn assert_error<T: std::error::Error>() {}
        assert_error::<MyErrno>();

        if FIRST.swap(false, ::std::sync::atomic::Ordering::SeqCst) {
            Ok(Err(MyErrno::B))
        } else {
            Ok(Ok(()))
        }
    }

    async fn list_typedefs(
        &self,
        _cx: Ctx,
        a: ListTypedef,
        c: ListTypedef3,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<(ListTypedef2, ListTypedef3)> {
        assert_eq!(a, "typedef1");
        assert_eq!(c.len(), 1);
        assert_eq!(c[0], "typedef2");
        Ok((
            ::wit_bindgen_wrpc::bytes::Bytes::from_static(b"typedef3"),
            vec!["typedef4".to_string()],
        ))
    }

    async fn list_of_variants(
        &self,
        _cx: Ctx,
        bools: Vec<bool>,
        results: Vec<::core::result::Result<(), ()>>,
        enums: Vec<MyErrno>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<(
        Vec<bool>,
        Vec<::core::result::Result<(), ()>>,
        Vec<MyErrno>,
    )> {
        assert_eq!(bools, [true, false]);
        assert_eq!(results, [Ok(()), Err(())]);
        assert_eq!(enums, [MyErrno::Success, MyErrno::A]);
        Ok((
            vec![false, true],
            vec![Err(()), Ok(())],
            vec![MyErrno::A, MyErrno::B],
        ))
    }
}
