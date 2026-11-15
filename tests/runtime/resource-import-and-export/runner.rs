use crate::client::test::resource_import_and_export::test::Thing;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    let thing1 = Thing::new(tx, rx, 42).await?;

    // 42 + 1 (constructor) + 1 (constructor) + 2 (foo) + 2 (foo)
    let (tx, rx) = wrpc.connect().await?;
    let v = Thing::foo(tx, rx, &thing1.as_borrow()).await?;
    assert_eq!(v, 48);

    // 33 + 3 (bar) + 3 (bar) + 2 (foo) + 2 (foo)
    let (tx, rx) = wrpc.connect().await?;
    Thing::bar(tx, rx, &thing1.as_borrow(), 33).await?;
    let (tx, rx) = wrpc.connect().await?;
    let v = Thing::foo(tx, rx, &thing1.as_borrow()).await?;
    assert_eq!(v, 43);

    let (tx, rx) = wrpc.connect().await?;
    let thing2 = Thing::new(tx, rx, 81).await?;
    let (tx, rx) = wrpc.connect().await?;
    let thing3 = Thing::baz(tx, rx, &thing1, &thing2).await?;
    let (tx, rx) = wrpc.connect().await?;
    let v = Thing::foo(tx, rx, &thing3.as_borrow()).await?;
    assert_eq!(
        v,
        33 + 3 + 3 + 81 + 1 + 1 + 2 + 2 + 4 + 1 + 2 + 4 + 1 + 1 + 2 + 2
    );
    Ok(())
}
