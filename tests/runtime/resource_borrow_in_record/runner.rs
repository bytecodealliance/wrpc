use crate::client::test::resource_borrow_in_record::to_test::{test, Foo, Thing};

pub async fn run(
    wrpc: &crate::Client,
) -> ::anyhow::Result<()> {
    let mut clt = wrpc.connect().await?;
    let thing1 = Thing::new(&mut clt, "Bonjour").await?;
    let thing2 = Thing::new(&mut clt, "mon cher").await?;
    let things = test(
        &mut clt,
        &[
            Foo {
                thing: thing1.as_borrow(),
            },
            Foo {
                thing: thing2.as_borrow(),
            },
        ],
    )
    .await?;
    let mut result = Vec::new();
    for thing in &things {
        result.push(Thing::get(&mut clt, &thing.as_borrow()).await?);
    }
    assert_eq!(result, ["Bonjour new test get", "mon cher new test get"]);
    Ok(())
}
