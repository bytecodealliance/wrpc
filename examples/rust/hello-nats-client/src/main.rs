use anyhow::Context as _;
use clap::Parser;
use tokio::sync::mpsc;
use url::Url;

mod bindings {
    wit_bindgen_wrpc::generate!();
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// NATS.io URL to connect to
    #[arg(short, long, default_value = "nats://127.0.0.1:4222")]
    nats: Url,

    /// Prefixes to invoke `wrpc-examples:hello/handler.hello` on
    prefixes: Vec<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let Args { nats, prefixes } = Args::parse();

    let nats = connect(nats)
        .await
        .context("failed to connect to NATS.io")?;
    for prefix in prefixes {
        let wrpc = wrpc_transport_nats::Client::new(nats.clone(), prefix.clone());
        let hello = bindings::wrpc_examples::hello::handler::hello(&wrpc)
            .await
            .context("failed to invoke `wrpc-examples.hello/handler.hello`")?;
        eprintln!("{prefix}: {hello}");
    }
    Ok(())
}

/// Connect to NATS.io server and ensure that the connection is fully established before
/// returning the resulting [`async_nats::Client`]
async fn connect(url: Url) -> anyhow::Result<async_nats::Client> {
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
