use anyhow::Context as _;
use wrpc_webtransport::Client;

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn loopback() -> anyhow::Result<()> {
    let srv = wrpc_webtransport::Server::new();
    wrpc_test::with_webtransport(|cc, sc| async move {
        wrpc_test::assert_single_invocation(&Client::from(cc), &srv, async {
            let (tx, rx) = sc.accept_bi().await.context("failed to accept stream")?;
            srv.accept((), tx, rx)
                .await
                .context("failed to accept invocation")
        })
        .await
    })
    .await
}
