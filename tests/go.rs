use anyhow::{Context, ensure};
use tokio::process::Command;
use tracing::instrument;

pub mod common;

#[instrument(ret)]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn go_bindgen() -> anyhow::Result<()> {
    use core::time::Duration;

    use anyhow::{anyhow, bail};
    use tokio::fs;
    use tokio::time::sleep;
    use tracing::info;

    if let Err(err) = fs::remove_dir_all("tests/go/bindings").await {
        match err.kind() {
            std::io::ErrorKind::NotFound => {}
            _ => bail!(anyhow!(err).context("failed to remove `test/go/bindings`")),
        }
    }
    let status = Command::new("go")
        .current_dir("tests/go")
        .args(["generate", "./..."])
        .env("WIT_BINDGEN_WRPC", env!("CARGO_BIN_EXE_wit-bindgen-wrpc"))
        .kill_on_drop(true)
        .status()
        .await
        .context("failed to call `go generate`")?;
    ensure!(status.success(), "`go generate` failed");

    let status = Command::new("go")
        .current_dir("tests/go")
        .args(["test", "-v", "-failfast"])
        .kill_on_drop(true)
        .status()
        .await
        .context("failed to call `go test`")?;
    ensure!(status.success(), "`go test` failed");

    {
        let port = wrpc_test::free_port()
            .await
            .context("failed to find free port")?;
        let addr = format!("[::1]:{port}");

        info!("starting `sync-server-tcp`");
        let mut server = Command::new("go")
            .current_dir("tests/go")
            .args(["run", "./cmd/sync-server-tcp", &addr])
            .kill_on_drop(true)
            .spawn()
            .context("failed to run `sync-server-tcp`")?;

        let client = wrpc_transport::frame::tcp::Client::from(addr);

        // TODO: Remove the need for this
        sleep(Duration::from_secs(1)).await;

        common::assert_sync((), &client).await?;

        server
            .start_kill()
            .context("failed to kill `sync-server-tcp`")?;
        server
            .wait_with_output()
            .await
            .context("failed to wait for `sync-server-tcp` to exit")?;
    }

    {
        let port = wrpc_test::free_port()
            .await
            .context("failed to find free port")?;
        let addr = format!("[::1]:{port}");

        info!("starting `async-server-tcp`");
        let mut server = Command::new("go")
            .current_dir("tests/go")
            .args(["run", "./cmd/async-server-tcp", &addr])
            .kill_on_drop(true)
            .spawn()
            .context("failed to run `async-server-tcp`")?;

        // TODO: Remove the need for this
        sleep(Duration::from_secs(1)).await;

        let client = wrpc_transport::frame::tcp::Client::from(addr);
        common::assert_async((), &client).await?;

        server
            .start_kill()
            .context("failed to kill `async-server-tcp`")?;
        server
            .wait_with_output()
            .await
            .context("failed to wait for `async-server-tcp` to exit")?;
    }

    Ok(())
}

#[instrument(ret)]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn go() -> anyhow::Result<()> {
    let status = Command::new("go")
        .current_dir("go")
        .args(["test", "-v", "-failfast", "./..."])
        .kill_on_drop(true)
        .status()
        .await
        .context("failed to call `go test`")?;
    ensure!(status.success(), "`go test` failed");
    Ok(())
}
