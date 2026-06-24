use crate::server::exports::test::list_in_variant::to_test::*;

#[derive(Clone)]
pub struct Component;

impl<Ctx: Send> crate::server::exports::test::list_in_variant::to_test::Handler<Ctx> for Component {
    async fn list_in_option(
        &self,
        _cx: Ctx,
        data: Option<Vec<String>>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<String> {
        Ok(match data {
            Some(list) => list.join(","),
            None => "none".to_string(),
        })
    }

    async fn list_in_variant(
        &self,
        _cx: Ctx,
        data: PayloadOrEmpty,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<String> {
        Ok(match data {
            PayloadOrEmpty::WithData(list) => list.join(","),
            PayloadOrEmpty::Empty => "empty".to_string(),
        })
    }

    async fn list_in_result(
        &self,
        _cx: Ctx,
        data: Result<Vec<String>, String>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<String> {
        Ok(match data {
            Ok(list) => list.join(","),
            Err(e) => format!("err:{e}"),
        })
    }

    async fn list_in_option_with_return(
        &self,
        _cx: Ctx,
        data: Option<Vec<String>>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<Summary> {
        Ok(match data {
            Some(list) => Summary {
                count: list.len() as u32,
                label: list.join(","),
            },
            None => Summary {
                count: 0,
                label: "none".to_string(),
            },
        })
    }

    async fn top_level_list(
        &self,
        _cx: Ctx,
        items: Vec<String>,
    ) -> ::wit_bindgen_wrpc::anyhow::Result<String> {
        Ok(items.join(","))
    }
}
