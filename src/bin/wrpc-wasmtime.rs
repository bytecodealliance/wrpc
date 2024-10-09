#[tokio::main]
async fn main() -> anyhow::Result<()> {
    wrpc_wasmtime_cli::run().await
}
