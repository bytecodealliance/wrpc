use tokio::sync::oneshot;
use tokio::{join, try_join};

use crate::client::wrpc_test::big_tuples::to_test::{many_params, nested_future, roundtrip};

pub async fn run(wrpc: &crate::Client) -> ::anyhow::Result<()> {
    let expected = Tuple17(
        0,
        1,
        2,
        3,
        4,
        5,
        6,
        7,
        8,
        9,
        10,
        11,
        12,
        13,
        14,
        15,
        "foo".to_string(),
    );
    let clt = wrpc.connect().await?;
    let a = roundtrip(clt, &[expected.clone()]).await?;
    assert_eq!(a, [expected.clone()]);

    let (tx, rx) = oneshot::channel();
    let clt = wrpc.connect().await?;
    let ((), v) = join!(async { tx.send(16).unwrap() }, async {
        let (fut, io) = nested_future(
            clt,
            Tuple17(
                0,
                1,
                2,
                3,
                4,
                5,
                6,
                7,
                8,
                9,
                10,
                11,
                12,
                13,
                14,
                15,
                Box::pin(async move { rx.await.unwrap() }),
            ),
        )
        .await?;
        let (v, ()) = try_join!(async { Ok(fut.await) }, io)?;
        ::anyhow::Ok(v)
    });
    let v = v?;
    assert_eq!(v, 16);

    let clt = wrpc.connect().await?;
    let a = many_params(
        clt, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, "foo",
    )
    .await?;
    assert_eq!(a, "foo");
    Ok(())
}
