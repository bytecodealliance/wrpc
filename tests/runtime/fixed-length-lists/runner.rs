//@ wasmtime-flags = '-Wcomponent-model-fixed-length-lists'

use crate::client::test::fixed_length_lists::to_test::*;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    list_param(wrpc, (), &[1, 2, 3, 4]).await?;
    list_param2(wrpc, (), &[[1, 2], [3, 4]]).await?;
    list_param3(
        wrpc,
        (),
        &[
            -1, 2, -3, 4, -5, 6, -7, 8, -9, 10, -11, 12, -13, 14, -15, 16, -17, 18, -19, 20,
        ],
    )
    .await?;

    assert_eq!(
        list_result(wrpc, ()).await?,
        [b'0', b'1', b'A', b'B', b'a', b'b', 128, 255]
    );

    assert_eq!(
        list_minmax16(wrpc, (), &[0, 1024, 32768, 65535], &[1, 2048, -32767, -2]).await?,
        ([0, 1024, 32768, 65535], [1, 2048, -32767, -2])
    );

    assert_eq!(
        list_minmax_float(wrpc, (), &[2.0, -42.0], &[0.25, -0.125]).await?,
        ([2.0, -42.0], [0.25, -0.125])
    );

    assert_eq!(
        list_roundtrip(
            wrpc,
            (),
            &[b'a', b'b', b'c', b'd', 0, 1, 2, 3, b'A', b'B', b'Y', b'Z']
        )
        .await?,
        [b'a', b'b', b'c', b'd', 0, 1, 2, 3, b'A', b'B', b'Y', b'Z']
    );

    assert_eq!(
        nested_roundtrip(
            wrpc,
            (),
            &[[1, 5], [42, 1_000_000]],
            &[[-1, 3], [-2_000_000, 4711]]
        )
        .await?,
        ([[1, 5], [42, 1_000_000]], [[-1, 3], [-2_000_000, 4711]])
    );

    assert_eq!(
        large_roundtrip(
            wrpc,
            (),
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

    let result =
        nightmare_on_cpp(wrpc, (), &[Nested { l: [1, -1] }, Nested { l: [2, -2] }]).await?;
    assert_eq!(result[0].l, [1, -1]);
    assert_eq!(result[1].l, [2, -2]);
    Ok(())
}
