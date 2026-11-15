//@ wasmtime-flags = '-Wcomponent-model-fixed-length-lists'

use crate::client::test::fixed_length_lists::to_test::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    list_param(tx, rx, &[1, 2, 3, 4]).await?;
    let (tx, rx) = wrpc.connect().await?;
    list_param2(tx, rx, &[[1, 2], [3, 4]]).await?;
    let (tx, rx) = wrpc.connect().await?;
    list_param3(
        tx,
        rx,
        &[
            -1, 2, -3, 4, -5, 6, -7, 8, -9, 10, -11, 12, -13, 14, -15, 16, -17, 18, -19, 20,
        ],
    )
    .await?;

    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        list_result(tx, rx).await?,
        [b'0', b'1', b'A', b'B', b'a', b'b', 128, 255]
    );

    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        list_minmax16(tx, rx, &[0, 1024, 32768, 65535], &[1, 2048, -32767, -2]).await?,
        ([0, 1024, 32768, 65535], [1, 2048, -32767, -2])
    );

    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        list_minmax_float(tx, rx, &[2.0, -42.0], &[0.25, -0.125]).await?,
        ([2.0, -42.0], [0.25, -0.125])
    );

    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        list_roundtrip(
            tx,
            rx,
            &[b'a', b'b', b'c', b'd', 0, 1, 2, 3, b'A', b'B', b'Y', b'Z']
        )
        .await?,
        [b'a', b'b', b'c', b'd', 0, 1, 2, 3, b'A', b'B', b'Y', b'Z']
    );

    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        nested_roundtrip(
            tx,
            rx,
            &[[1, 5], [42, 1_000_000]],
            &[[-1, 3], [-2_000_000, 4711]]
        )
        .await?,
        ([[1, 5], [42, 1_000_000]], [[-1, 3], [-2_000_000, 4711]])
    );

    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        large_roundtrip(
            tx,
            rx,
            &[[1, 5], [42, 1_000_000]],
            &[
                [-1, 3, -2, 4],
                [-2_000_000, 4711, 99_999, -5],
                [-6, 7, 8, -9],
                [50, -5, 500, -5000],
            ],
        )
        .await?,
        (
            [[1, 5], [42, 1_000_000]],
            [
                [-1, 3, -2, 4],
                [-2_000_000, 4711, 99_999, -5],
                [-6, 7, 8, -9],
                [50, -5, 500, -5000]
            ]
        )
    );

    let (tx, rx) = wrpc.connect().await?;
    let result =
        nightmare_on_cpp(tx, rx, &[Nested { l: [1, -1] }, Nested { l: [2, -2] }]).await?;
    assert_eq!(result[0].l, [1, -1]);
    assert_eq!(result[1].l, [2, -2]);
    Ok(())
}
