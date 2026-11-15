#![cfg(all(unix, feature = "net"))]

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn loopback() -> anyhow::Result<()> {
    wrpc_test::serve_unix(async |path, lis| {
        let mut srv = wrpc_test::unix_incoming(lis);
        let connect = || wrpc_test::unix_streams(path.clone());
        wrpc_test::assert_single_invocation(connect, &mut srv).await?;
        wrpc_test::assert_invalid_data_resets(connect, srv).await
    })
    .await
}
