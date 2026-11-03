//@ args = '--ownership borrowing'

use crate::client::test::ownership::both_list_and_resource;

impl PartialEq for thing_in_and_out::Thing {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.value == other.value
    }
}

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let value = &[&["foo", "bar"] as &[_]] as &[_];
    let res = lists::foo(wrpc, (), value).await?;
    assert_eq!(vec![vec!["foo".to_owned(), "bar".to_owned()]], res);

    thing_in::bar(
        wrpc,
        (),
        thing_in::Thing {
            name: "thing 1",
            value: &["some value", "another value"],
        },
    )
    .await?;

    let value = thing_in_and_out::Thing {
        name: "thing 1".to_owned(),
        value: vec!["some value".to_owned(), "another value".to_owned()],
    };
    let res = thing_in_and_out::baz(wrpc, (), &value).await?;
    assert_eq!(value, res);

    let strings = vec!["foo", "bar", "baz"];
    let resource = both_list_and_resource::TheResource::new(wrpc, (), &strings).await?;
    both_list_and_resource::list_and_resource(
        wrpc,
        (),
        both_list_and_resource::Thing {
            a: strings.iter().map(|s| s.to_string()).collect(),
            b: resource,
        },
    )
    .await?;
    Ok(())
}
