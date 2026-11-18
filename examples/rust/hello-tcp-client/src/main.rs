use anyhow::Context as _;
use clap::Parser;
use tokio::net::TcpStream;
use wit_bindgen_wrpc::tokio_util::codec::{FramedRead, FramedWrite};
use wit_bindgen_wrpc::wrpc_transport::{FrameDecoder, FrameEncoder};

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
    /// Address to invoke `wrpc-examples:hello/handler.hello` on
    #[arg(default_value = "[::1]:7761")]
    addr: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let Args { addr } = Args::parse();
    let mut stream = TcpStream::connect(&addr)
        .await
        .with_context(|| format!("failed to connect to `{addr}`"))?;
    stream
        .set_nodelay(true)
        .context("failed to set TCP_NODELAY")?;
    let (rx, tx) = stream.split();
    let tx = FramedWrite::new(tx, FrameEncoder);
    let rx = FramedRead::new(rx, FrameDecoder::default());
    let hello = bindings::wrpc_examples::hello::handler::hello(tx, rx)
        .await
        .context("failed to invoke `wrpc-examples.hello/handler.hello`")?;
    eprintln!("{hello}");
    Ok(())
}
