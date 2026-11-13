#![cfg(all(unix, feature = "net"))]

use wrpc_transport::unix;

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn loopback() -> anyhow::Result<()> {
    wrpc_test::serve_unix(async |clt, srv| {
        wrpc_test::assert_single_invocation((), &clt, unix::incoming(srv)).await?;
        wrpc_test::assert_invalid_data_resets((), &clt, unix::incoming(srv)).await
    })
    .await
}
