use crate::client::test::resource_borrow_in_record::to_test::{test, Foo, Thing};

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (mut tx, mut rx) = wrpc.connect().await?;
    let thing1 = Thing::new(&mut tx, &mut rx, "Bonjour").await?;
    let thing2 = Thing::new(&mut tx, &mut rx, "mon cher").await?;
    let things = test(
        &mut tx,
        &mut rx,
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
        result.push(Thing::get(&mut tx, &mut rx, &thing.as_borrow()).await?);
    }
    assert_eq!(result, ["Bonjour new test get", "mon cher new test get"]);
    Ok(())
}
