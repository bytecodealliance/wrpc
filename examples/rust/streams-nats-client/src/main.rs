use core::time::Duration;

use anyhow::Context as _;
use bytes::Bytes;
use clap::Parser;
use futures::{stream, StreamExt as _};
use tokio::sync::mpsc;
use tokio::{time, try_join};
use tokio_stream::wrappers::IntervalStream;
use tracing::debug;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use url::Url;

mod bindings {
    wit_bindgen_wrpc::generate!({
        with: {
            "wrpc-examples:streams/handler": generate
        }
    });
}

use bindings::wrpc_examples::streams::handler::{echo, Req};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// NATS.io URL to connect to
    #[arg(short, long, default_value = "nats://127.0.0.1:4222")]
    nats: Url,

    /// Prefixes to invoke `wrpc-examples:streams/handler.echo` on
    #[arg(default_value = "rust")]
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
        let numbers = Box::pin(
            stream::iter(1..)
                .take(10)
                .zip(IntervalStream::new(time::interval(Duration::from_secs(1))))
                .map(|(i, _)| i)
                .ready_chunks(10),
        );

        // `stream<u8>` items are chunked using [`Bytes`]
        let bytes = Box::pin(
            stream::iter(b"foo bar baz")
                .zip(IntervalStream::new(time::interval(Duration::from_secs(1))))
                .map(|(i, _)| *i)
                .ready_chunks(10)
                .map(Bytes::from),
        );

        let wrpc = wrpc_transport_nats::Client::new(nats.clone(), prefix.clone(), None);
        let (mut numbers, mut bytes, io) = echo(&wrpc, None, Req { numbers, bytes })
            .await
            .context("failed to invoke `wrpc-examples:streams/handler.echo`")?;
        try_join!(
            async {
                if let Some(io) = io {
                    debug!("performing async I/O");
                    io.await.context("failed to complete async I/O")
                } else {
                    Ok(())
                }
            },
            async {
                while let Some(item) = numbers.next().await {
                    eprintln!("numbers: {item:?}");
                }
                Ok(())
            },
            async {
                while let Some(item) = bytes.next().await {
                    eprintln!("bytes: {item:?}");
                }
                Ok(())
            }
        )?;
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
