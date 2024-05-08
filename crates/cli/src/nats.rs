use anyhow::Context as _;
use tokio::sync::mpsc;
use url::Url;

pub const DEFAULT_URL: &str = "nats://127.0.0.1:4222";

/// Connect to NATS.io server and ensure that the connection is fully established before
/// returning the resulting [`async_nats::Client`]
pub async fn connect(url: Url) -> anyhow::Result<async_nats::Client> {
    let (conn_tx, mut conn_rx) = mpsc::channel(1);
    let client = async_nats::connect_with_options(
        String::from(url),
        async_nats::ConnectOptions::new()
            .retry_on_initial_connect()
            .event_callback(move |event| {
                let conn_tx = conn_tx.clone();
                async move {
                    if let async_nats::Event::Connected = event {
                        conn_tx
                            .send(())
                            .await
                            .expect("failed to send NATS.io server connection notification");
                    }
                }
            }),
    )
    .await
    .context("failed to connect to NATS.io server")?;
    conn_rx
        .recv()
        .await
        .context("failed to await NATS.io server connection to be established")?;
    Ok(client)
}
