use crate::client::test::resource_import_and_export::test::Thing;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (mut tx, mut rx) = wrpc.connect().await?;
    let thing1 = Thing::new(&mut tx, &mut rx, 42).await?;

    // 42 + 1 (constructor) + 1 (constructor) + 2 (foo) + 2 (foo)
    let v = Thing::foo(&mut tx, &mut rx, &thing1.as_borrow()).await?;
    assert_eq!(v, 48);

    // 33 + 3 (bar) + 3 (bar) + 2 (foo) + 2 (foo)
    Thing::bar(&mut tx, &mut rx, &thing1.as_borrow(), 33).await?;
    let v = Thing::foo(&mut tx, &mut rx, &thing1.as_borrow()).await?;
    assert_eq!(v, 43);

    let thing2 = Thing::new(&mut tx, &mut rx, 81).await?;
    let thing3 = Thing::baz(&mut tx, &mut rx, &thing1, &thing2).await?;
    let v = Thing::foo(&mut tx, &mut rx, &thing3.as_borrow()).await?;
    assert_eq!(
        v,
        33 + 3 + 3 + 81 + 1 + 1 + 2 + 2 + 4 + 1 + 2 + 4 + 1 + 1 + 2 + 2
    );
    Ok(())
}
