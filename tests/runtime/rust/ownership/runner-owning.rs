//@ args = '--ownership owning'

use crate::client::test::ownership::both_list_and_resource;

impl PartialEq for thing_in_and_out::Thing {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.value == other.value
    }
}

pub async fn run(
    wrpc: &crate::Client,
) -> ::anyhow::Result<()> {
    let value = vec![vec!["foo".to_owned(), "bar".to_owned()]];
    let mut clt = wrpc.connect().await?;
    let res = lists::foo(&mut clt, &value).await?;
    assert_eq!(value, res);

    thing_in::bar(
        &mut clt,
        &thing_in::Thing {
            name: "thing 1".to_owned(),
            value: vec!["some value".to_owned(), "another value".to_owned()],
        },
    )
    .await?;

    let value = thing_in_and_out::Thing {
        name: "thing 1".to_owned(),
        value: vec!["some value".to_owned(), "another value".to_owned()],
    };
    let res = thing_in_and_out::baz(&mut clt, &value).await?;
    assert_eq!(value, res);

    let strings = vec!["foo".to_string(), "bar".to_string(), "baz".to_string()];
    let resource = both_list_and_resource::TheResource::new(&mut clt, &strings).await?;
    both_list_and_resource::list_and_resource(
        &mut clt,
        both_list_and_resource::Thing {
            a: strings,
            b: resource,
        },
    )
    .await?;
    Ok(())
}
