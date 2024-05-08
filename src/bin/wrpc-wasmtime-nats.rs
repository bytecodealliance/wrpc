#[tokio::main]
async fn main() -> anyhow::Result<()> {
    wrpc_wasmtime_nats_cli::run().await
}
