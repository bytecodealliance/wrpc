use crate::client::test::resource_aggregates::to_test::{
    foo, Thing, R1, R2, R3, V1, V2,
};

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let mut clt = wrpc.connect().await?;
    let mut thing = async |n| Thing::new(&mut clt, n).await;
    let thing0 = thing(0).await?;
    let thing1 = thing(1).await?;
    let thing2 = thing(2).await?;
    let thing3 = thing(3).await?;
    let thing4 = thing(4).await?;
    let thing5 = thing(5).await?;
    let thing6 = thing(6).await?;
    let thing7 = thing(7).await?;
    let thing8 = thing(8).await?;
    let thing9 = thing(9).await?;
    let thing10 = thing(10).await?;
    let thing11 = thing(11).await?;
    let thing12 = thing(12).await?;
    let thing13 = thing(13).await?;
    let thing14 = thing(14).await?;
    let thing15 = thing(15).await?;
    let thing16 = thing(16).await?;

    let result = foo(
        &mut clt,
        &R1 { thing: thing0 },
        &R2 {
            thing: thing1.as_borrow(),
        },
        &R3 {
            thing1: thing2.as_borrow(),
            thing2: thing3,
        },
        (thing4, R1 { thing: thing5 }),
        (thing6.as_borrow(),),
        &V1::Thing(thing7),
        &V2::Thing(thing8.as_borrow()),
        &[thing9, thing10],
        &[thing11.as_borrow(), thing12.as_borrow()],
        Some(thing13),
        Some(thing14.as_borrow()),
        &Ok(thing15),
        &Ok(thing16.as_borrow()),
    )
    .await?;
    assert_eq!(result, (0..17).map(|i| i + 1).sum::<u32>() + 3);
    Ok(())
}
