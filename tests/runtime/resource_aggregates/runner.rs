use crate::client::test::resource_aggregates::to_test::{
    foo, Thing, R1, R2, R3, V1, V2,
};

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let thing = async |n| {
        let (tx, rx) = wrpc.connect().await?;
        Thing::new(tx, rx, n).await
    };
    let thing1 = thing(1).await?;
    let thing2 = thing(2).await?;
    let thing6 = thing(6).await?;
    let thing8 = thing(8).await?;
    let thing11 = thing(11).await?;
    let thing12 = thing(12).await?;
    let thing14 = thing(14).await?;
    let thing16 = thing(16).await?;

    let (tx, rx) = wrpc.connect().await?;
    let result = foo(
        tx,
        rx,
        &R1 {
            thing: thing(0).await?,
        },
        &R2 {
            thing: thing1.as_borrow(),
        },
        &R3 {
            thing1: thing2.as_borrow(),
            thing2: thing(3).await?,
        },
        (
            thing(4).await?,
            R1 {
                thing: thing(5).await?,
            },
        ),
        (thing6.as_borrow(),),
        &V1::Thing(thing(7).await?),
        &V2::Thing(thing8.as_borrow()),
        &[
            thing(9).await?,
            thing(10).await?,
        ],
        &[thing11.as_borrow(), thing12.as_borrow()],
        Some(thing(13).await?),
        Some(thing14.as_borrow()),
        &Ok(thing(15).await?),
        &Ok(thing16.as_borrow()),
    )
    .await?;
    assert_eq!(result, (0..17).map(|i| i + 1).sum::<u32>() + 3);
    Ok(())
}
