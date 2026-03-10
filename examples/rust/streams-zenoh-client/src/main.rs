use anyhow::Context as _;
use bytes::Bytes;
use clap::Parser;
use core::time::Duration;
use futures::{stream, StreamExt as _};
use std::sync::Arc;
use tokio::{time, try_join};
use tokio_stream::wrappers::IntervalStream;
use tracing::debug;

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
    /// Prefixes to invoke `wrpc-examples:streams/handler.echo` on
    #[arg(default_value = "rust")]
    prefixes: Vec<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let Args { prefixes } = Args::parse();

    let session = wrpc_cli::zenoh::connect()
        .await
        .context("failed to connect to zenoh")?;

    let arc_session = Arc::new(session);

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

        // Client creation moved here from top
        let wrpc = wrpc_transport_zenoh::Client::new(arc_session.clone(), prefix)
            .await
            .context("failed to construct transport client")?;

        let (mut numbers, mut bytes, io) = echo(&wrpc, (), Req { numbers, bytes })
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
