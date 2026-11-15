use crate::client::test::resource_alias_redux::resource_alias1 as a1;
use crate::client::test::resource_alias_redux::resource_alias2 as a2;
use crate::client::the_test::test;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    let thing1 = a1::Thing::new(tx, rx, "Ni Hao").await?;
    let (tx, rx) = wrpc.connect().await?;
    let result = test(tx, rx, &[thing1]).await?;
    assert_eq!(result.len(), 1);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        a1::Thing::get(tx, rx, &result[0].as_borrow()).await?,
        "Ni Hao GuestThing GuestThing.get"
    );

    let (tx, rx) = wrpc.connect().await?;
    let thing2 = a1::Thing::new(tx, rx, "Ciao").await?;
    let (tx, rx) = wrpc.connect().await?;
    let result = a1::a(tx, rx, &a1::Foo { thing: thing2 }).await?;
    assert_eq!(result.len(), 1);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        a1::Thing::get(tx, rx, &result[0].as_borrow()).await?,
        "Ciao GuestThing GuestThing.get"
    );

    let (tx, rx) = wrpc.connect().await?;
    let thing3 = a1::Thing::new(tx, rx, "Ciao").await?;
    let (tx, rx) = wrpc.connect().await?;
    let thing4 = a1::Thing::new(tx, rx, "Aloha").await?;

    let (tx, rx) = wrpc.connect().await?;
    let result = a2::b(
        tx,
        rx,
        &a2::Foo { thing: thing3 },
        &a1::Foo { thing: thing4 },
    )
    .await?;
    assert_eq!(result.len(), 2);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        a1::Thing::get(tx, rx, &result[0].as_borrow()).await?,
        "Ciao GuestThing GuestThing.get"
    );
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        a1::Thing::get(tx, rx, &result[1].as_borrow()).await?,
        "Aloha GuestThing GuestThing.get"
    );
    Ok(())
}
