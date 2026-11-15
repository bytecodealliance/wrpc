use crate::client::test::resource_borrow_in_record::to_test::{test, Foo, Thing};

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    let thing1 = Thing::new(tx, rx, "Bonjour").await?;
    let (tx, rx) = wrpc.connect().await?;
    let thing2 = Thing::new(tx, rx, "mon cher").await?;
    let (tx, rx) = wrpc.connect().await?;
    let things = test(
        tx,
        rx,
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
        let (tx, rx) = wrpc.connect().await?;
        result.push(Thing::get(tx, rx, &thing.as_borrow()).await?);
    }
    assert_eq!(result, ["Bonjour new test get", "mon cher new test get"]);
    Ok(())
}
