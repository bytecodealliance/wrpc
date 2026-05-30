use anyhow::Context as _;
use bytes::Bytes;
use clap::Parser;
use futures::stream;
use tokio::try_join;
use tracing::debug;

mod bindings {
    wit_bindgen_wrpc::generate!({
        with: {
            "wrpc-examples:streams-io/handler": generate
        }
    });
}

use bindings::wrpc_examples::streams_io::handler::count;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Address to invoke `wrpc-examples:streams-io/handler.count` on
    #[arg(default_value = "[::1]:7761")]
    addr: String,

    /// Bytes to send through the stream
    #[arg(default_value = "hello from a wRPC stream<u8>")]
    payload: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let Args { addr, payload } = Args::parse();

    let wrpc = wrpc_transport::tcp::Client::from(&addr);

    // A `stream<u8>` is chunked using [`Bytes`]. The component server reads it
    // as a `wasi:io/streams.input-stream` and counts the bytes.
    let data = Box::pin(stream::iter([Bytes::from(payload.clone().into_bytes())]));

    let (total, io) = count(&wrpc, (), data)
        .await
        .context("failed to invoke `wrpc-examples:streams-io/handler.count`")?;

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
            eprintln!("sent {} bytes, server counted {total}", payload.len());
            Ok(())
        }
    )?;
    Ok(())
}
