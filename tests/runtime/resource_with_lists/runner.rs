use ::wit_bindgen_wrpc::bytes::Bytes;

use crate::client::test::resource_with_lists::test::Thing;

pub async fn run(
    wrpc: &crate::Client,
) -> ::anyhow::Result<()> {
    let mut clt = wrpc.connect().await?;
    let thing_instance = Thing::new(&mut clt, &Bytes::from_static(b"Hi")).await?;
    let v = Thing::foo(&mut clt, &thing_instance.as_borrow()).await?;
    assert_eq!(v, b"Hi Thing HostThing HostThing.foo Thing.foo".as_slice());
    Thing::bar(
        &mut clt,
        &thing_instance.as_borrow(),
        &Bytes::from_static(b"Hola"),
    )
    .await?;
    let v = Thing::foo(&mut clt, &thing_instance.as_borrow()).await?;
    assert_eq!(
        v,
        b"Hola Thing.bar HostThing.bar HostThing.foo Thing.foo".as_slice()
    );
    let v = Thing::baz(&mut clt, &Bytes::from_static(b"Ohayo Gozaimas")).await?;
    assert_eq!(
        v,
        b"Ohayo Gozaimas Thing.baz HostThing.baz Thing.baz again".as_slice()
    );
    Ok(())
}
