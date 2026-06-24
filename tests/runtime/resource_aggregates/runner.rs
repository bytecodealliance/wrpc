use crate::client::test::resource_aggregates::to_test::{
    foo, Thing, R1, R2, R3, V1, V2,
};

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let thing1 = Thing::new(wrpc, (), 1).await?;
    let thing2 = Thing::new(wrpc, (), 2).await?;
    let thing6 = Thing::new(wrpc, (), 6).await?;
    let thing8 = Thing::new(wrpc, (), 8).await?;
    let thing11 = Thing::new(wrpc, (), 11).await?;
    let thing12 = Thing::new(wrpc, (), 12).await?;
    let thing14 = Thing::new(wrpc, (), 14).await?;
    let thing16 = Thing::new(wrpc, (), 16).await?;

    let result = foo(
        wrpc,
        (),
        &R1 {
            thing: Thing::new(wrpc, (), 0).await?,
        },
        &R2 {
            thing: thing1.as_borrow(),
        },
        &R3 {
            thing1: thing2.as_borrow(),
            thing2: Thing::new(wrpc, (), 3).await?,
        },
        (
            Thing::new(wrpc, (), 4).await?,
            R1 {
                thing: Thing::new(wrpc, (), 5).await?,
            },
        ),
        (thing6.as_borrow(),),
        &V1::Thing(Thing::new(wrpc, (), 7).await?),
        &V2::Thing(thing8.as_borrow()),
        &[
            Thing::new(wrpc, (), 9).await?,
            Thing::new(wrpc, (), 10).await?,
        ],
        &[thing11.as_borrow(), thing12.as_borrow()],
        Some(Thing::new(wrpc, (), 13).await?),
        Some(thing14.as_borrow()),
        &Ok(Thing::new(wrpc, (), 15).await?),
        &Ok(thing16.as_borrow()),
    )
    .await?;
    assert_eq!(result, (0..17).map(|i| i + 1).sum::<u32>() + 3);
    Ok(())
}
