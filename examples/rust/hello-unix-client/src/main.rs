use std::path::PathBuf;

use anyhow::Context as _;
use clap::Parser;

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
    let wrpc = wrpc_transport::unix::Client::from(path.as_path());
    let hello = bindings::wrpc_examples::hello::handler::hello(&wrpc, ())
        .await
        .context("failed to invoke `wrpc-examples.hello/handler.hello`")?;
    eprintln!("{hello}");
    Ok(())
}
