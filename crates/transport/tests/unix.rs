#![cfg(all(unix, feature = "net"))]

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn loopback() -> anyhow::Result<()> {
    wrpc_test::serve_unix(async |path, lis| {
        let connect = || wrpc_test::unix_streams(path.clone());
        let accept = async || wrpc_test::unix_accept(&lis).await;
        let (tx, rx) = connect().await?;
        wrpc_test::assert_single_invocation(tx, rx, accept).await?;
        wrpc_test::assert_invalid_data_resets(connect, accept).await
    })
    .await
}
