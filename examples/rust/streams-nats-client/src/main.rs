use core::time::Duration;

use anyhow::Context as _;
use bytes::Bytes;
use clap::Parser;
use futures::{stream, StreamExt as _};
use tokio::{time, try_join};
use tokio_stream::wrappers::IntervalStream;
use tracing::debug;
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
    tracing_subscriber::fmt().init();

    let Args { nats, prefixes } = Args::parse();

    let nats = async_nats::connect_with_options(
        String::from(nats),
        async_nats::ConnectOptions::new().retry_on_initial_connect(),
    )
    .await
    .context("failed to connect to NATS.io server")?;

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

        let wrpc = wrpc_transport_nats::Client::new(nats.clone(), prefix, None)
            .await
            .context("failed to construct transport client")?;
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
