use crate::client::test::resource_borrow_in_record::to_test::{test, Foo, Thing};

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let thing1 = Thing::new(wrpc, (), "Bonjour").await?;
    let thing2 = Thing::new(wrpc, (), "mon cher").await?;
    let things = test(
        wrpc,
        (),
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
        result.push(Thing::get(wrpc, (), &thing.as_borrow()).await?);
    }
    assert_eq!(result, ["Bonjour new test get", "mon cher new test get"]);
    Ok(())
}
