use anyhow::{anyhow, bail, ensure, Context};
use tokio::fs;
use tokio::process::Command;

mod common;
use common::init;

#[tokio::test(flavor = "multi_thread")]
async fn go_bindgen() -> anyhow::Result<()> {
    init().await;

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
        .args(["test", "./..."])
        .kill_on_drop(true)
        .status()
        .await
        .context("failed to call `go test`")?;
    ensure!(status.success(), "`go test` failed");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn go() -> anyhow::Result<()> {
    init().await;

    let status = Command::new("go")
        .current_dir("go")
        .args(["test", "./..."])
        .kill_on_drop(true)
        .status()
        .await
        .context("failed to call `go test`")?;
    ensure!(status.success(), "`go test` failed");
    Ok(())
}
