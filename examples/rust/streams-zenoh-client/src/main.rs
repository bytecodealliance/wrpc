use core::time::Duration;
use std::sync::Arc;

use anyhow::Context as _;
use bytes::Bytes;
use futures::{stream, StreamExt as _};
use tokio::{time, try_join};
use tokio_stream::wrappers::IntervalStream;
use tracing::debug;
use zenoh::{Config};

mod bindings {
    wit_bindgen_wrpc::generate!({
        with: {
            "wrpc-examples:streams/handler": generate
        }
    });
}

use bindings::wrpc_examples::streams::handler::{echo, Req};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let cfg = Config::from_env().expect("Missing environment variable 'ZENOH_CONFIG'");

    let session = zenoh::open(cfg)
                            .await
                            .expect("Failed to open a Zenoh session");

    let arc_session = Arc::new(session);
    
    let prefix = Arc::<str>::from("rust"); 
    //let prefixes = vec!["Hello", "Zenoh", "streams!"]; // Expected was a string vec ... !

    //for prefix in prefixes {
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
        let wrpc = wrpc_transport_zenoh::Client::new(arc_session, prefix)
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
    //}
    Ok(())
}
