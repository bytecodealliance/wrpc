use crate::client::test::resource_alias_redux::resource_alias1 as a1;
use crate::client::test::resource_alias_redux::resource_alias2 as a2;
use crate::client::the_test::test;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let mut clt = wrpc.connect().await?;
    let thing1 = a1::Thing::new(&mut clt, "Ni Hao").await?;
    let result = test(&mut clt, &[thing1]).await?;
    assert_eq!(result.len(), 1);
    assert_eq!(
        a1::Thing::get(&mut clt, &result[0].as_borrow()).await?,
        "Ni Hao GuestThing GuestThing.get"
    );

    let thing2 = a1::Thing::new(&mut clt, "Ciao").await?;
    let result = a1::a(&mut clt, &a1::Foo { thing: thing2 }).await?;
    assert_eq!(result.len(), 1);
    assert_eq!(
        a1::Thing::get(&mut clt, &result[0].as_borrow()).await?,
        "Ciao GuestThing GuestThing.get"
    );

    let thing3 = a1::Thing::new(&mut clt, "Ciao").await?;
    let thing4 = a1::Thing::new(&mut clt, "Aloha").await?;

    let result = a2::b(
        &mut clt,
        &a2::Foo { thing: thing3 },
        &a1::Foo { thing: thing4 },
    )
    .await?;
    assert_eq!(result.len(), 2);
    assert_eq!(
        a1::Thing::get(&mut clt, &result[0].as_borrow()).await?,
        "Ciao GuestThing GuestThing.get"
    );
    assert_eq!(
        a1::Thing::get(&mut clt, &result[1].as_borrow()).await?,
        "Aloha GuestThing GuestThing.get"
    );
    Ok(())
}
