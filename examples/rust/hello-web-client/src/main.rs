use anyhow::Context as _;
use clap::Parser;
use url::Url;
use web_transport_quinn::ClientBuilder;

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
    #[arg(default_value = "https://localhost:4433")]
    addr: Url,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let Args { addr } = Args::parse();
    let ep = ClientBuilder::default()
        .dangerous()
        .with_no_certificate_verification()
        .context("failed to create endpoint")?;
    let session = ep
        .connect(addr)
        .await
        .context("failed to connect to server")?;
    let wrpc = wrpc_webtransport::Client::from(session);
    let hello = bindings::wrpc_examples::hello::handler::hello(&wrpc, ())
        .await
        .context("failed to invoke `wrpc-examples.hello/handler.hello`")?;
    eprintln!("{hello}");
    Ok(())
}
