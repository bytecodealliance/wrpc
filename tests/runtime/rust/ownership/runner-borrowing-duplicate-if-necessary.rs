//@ args = '--ownership borrowing-duplicate-if-necessary'

use crate::client::test::ownership::both_list_and_resource;

impl PartialEq for thing_in_and_out::ThingResult {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.value == other.value
    }
}

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let value = &[&["foo", "bar"] as &[_]] as &[_];
    let mut clt = wrpc.connect().await?;
    let res = lists::foo(&mut clt, value).await?;
    assert_eq!(vec![vec!["foo".to_owned(), "bar".to_owned()]], res);

    thing_in::bar(
        &mut clt,
        thing_in::Thing {
            name: "thing 1",
            value: &["some value", "another value"],
        },
    )
    .await?;

    let value = thing_in_and_out::ThingParam {
        name: "thing 1",
        value: &["some value", "another value"],
    };
    let res = thing_in_and_out::baz(&mut clt, value).await?;
    assert_eq!(
        thing_in_and_out::ThingResult {
            name: "thing 1".to_owned(),
            value: vec!["some value".to_owned(), "another value".to_owned()],
        },
        res
    );

    let strings = vec!["foo", "bar", "baz"];
    let resource = both_list_and_resource::TheResource::new(&mut clt, &strings).await?;
    both_list_and_resource::list_and_resource(
        &mut clt,
        both_list_and_resource::Thing {
            a: strings.iter().map(|s| s.to_string()).collect(),
            b: resource,
        },
    )
    .await?;
    Ok(())
}
