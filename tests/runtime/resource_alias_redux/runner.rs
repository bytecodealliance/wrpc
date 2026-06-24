use crate::runner::test::resource_alias_redux::resource_alias1 as a1;
use crate::runner::test::resource_alias_redux::resource_alias2 as a2;
use crate::runner::the_test::test;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let thing1 = a1::Thing::new(wrpc, (), "Ni Hao").await?;
    let result = test(wrpc, (), &[thing1]).await?;
    assert_eq!(result.len(), 1);
    assert_eq!(
        a1::Thing::get(wrpc, (), &result[0].as_borrow()).await?,
        "Ni Hao GuestThing GuestThing.get"
    );

    let thing2 = a1::Thing::new(wrpc, (), "Ciao").await?;
    let result = a1::a(wrpc, (), &a1::Foo { thing: thing2 }).await?;
    assert_eq!(result.len(), 1);
    assert_eq!(
        a1::Thing::get(wrpc, (), &result[0].as_borrow()).await?,
        "Ciao GuestThing GuestThing.get"
    );

    let thing3 = a1::Thing::new(wrpc, (), "Ciao").await?;
    let thing4 = a1::Thing::new(wrpc, (), "Aloha").await?;

    let result = a2::b(
        wrpc,
        (),
        &a2::Foo { thing: thing3 },
        &a1::Foo { thing: thing4 },
    )
    .await?;
    assert_eq!(result.len(), 2);
    assert_eq!(
        a1::Thing::get(wrpc, (), &result[0].as_borrow()).await?,
        "Ciao GuestThing GuestThing.get"
    );
    assert_eq!(
        a1::Thing::get(wrpc, (), &result[1].as_borrow()).await?,
        "Aloha GuestThing GuestThing.get"
    );
    Ok(())
}
