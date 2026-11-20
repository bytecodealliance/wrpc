use std::path::PathBuf;

use anyhow::Context as _;
use clap::Parser;
use tokio::net::UnixStream;
use wit_bindgen_wrpc::wrpc_transport::Framed;

mod bindings {
    wit_bindgen_wrpc::generate!({
        with: {
            "wrpc-examples:hello/handler": generate
        }
    });
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to invoke `wrpc-examples:hello/handler.hello` on
    #[arg(default_value = "/tmp/wrpc/hello.sock")]
    path: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let Args { path } = Args::parse();
    let mut stream = UnixStream::connect(&path)
        .await
        .with_context(|| format!("failed to connect to `{}`", path.display()))?;
    let (rx, tx) = stream.split();
    let hello = bindings::wrpc_examples::hello::handler::hello(Framed::new(tx, rx))
        .await
        .context("failed to invoke `wrpc-examples.hello/handler.hello`")?;
    eprintln!("{hello}");
    Ok(())
}
