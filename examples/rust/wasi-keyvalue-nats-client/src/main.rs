use anyhow::{ensure, Context as _};
use bytes::Bytes;
use clap::Parser;
use tokio::sync::mpsc;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use url::Url;

mod bindings {
    wit_bindgen_wrpc::generate!({
       with: {
           "wasi:keyvalue/store@0.2.0-draft": generate
       }
    });
}

use bindings::wasi::keyvalue::store;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// NATS.io URL to connect to
    #[arg(short, long, default_value = "nats://127.0.0.1:4222")]
    nats: Url,

    /// Prefixes to invoke `wasi:keyvalue/store` functions on
    prefixes: Vec<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with(tracing_subscriber::fmt::layer().compact().without_time())
        .init();

    let Args { nats, prefixes } = Args::parse();

    let nats = connect(nats)
        .await
        .context("failed to connect to NATS.io")?;
    for prefix in prefixes {
        let wrpc = wrpc_transport_nats::Client::new(nats.clone(), prefix.clone(), None);
        let bucket = store::open(&wrpc, None, "example")
            .await
            .context("failed to invoke `open`")?
            .context("failed to open empty bucket")?;
        store::Bucket::set(&wrpc, None, &bucket.as_borrow(), "foo", &Bytes::from("bar"))
            .await
            .context("failed to invoke `set`")?
            .context("failed to set `foo`")?;
        let ok = store::Bucket::exists(&wrpc, None, &bucket.as_borrow(), "foo")
            .await
            .context("failed to invoke `exists`")?
            .context("failed to check if `foo` exists")?;
        ensure!(ok);
        let v = store::Bucket::get(&wrpc, None, &bucket.as_borrow(), "foo")
            .await
            .context("failed to invoke `get`")?
            .context("failed to get `foo`")?;
        ensure!(v.as_deref() == Some(b"bar".as_slice()));

        let store::KeyResponse { keys, cursor: _ } =
            store::Bucket::list_keys(&wrpc, None, &bucket.as_borrow(), None)
                .await
                .context("failed to invoke `list-keys`")?
                .context("failed to list keys")?;
        for key in keys {
            println!("{prefix} key: {key}");
        }
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
