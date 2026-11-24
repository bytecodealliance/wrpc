use std::net::SocketAddr;

use anyhow::{Context as _, ensure};
use clap::{Parser, ValueEnum};
use http::{Request, Response, Uri};
use tower::service_fn;
use wrpc::http::{Client, OutgoingParams};

mod bindings {
    wrpc::generate!({
        with: {
            "wrpc-examples:hello/handler": generate
        }
    });
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Version {
    #[value(name = "1.1")]
    Http1,
    #[value(name = "2")]
    Http2,
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// HTTP version to use
    #[arg(long, value_enum, default_value_t = Version::Http2)]
    http: Version,

    /// Address to invoke `wrpc-examples:hello/handler.hello` on
    #[arg(default_value = "[::1]:7763")]
    addr: SocketAddr,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let Args { http, addr } = Args::parse();

    let clt = match http {
        Version::Http1 => reqwest::Client::builder().http1_only(),
        Version::Http2 => reqwest::Client::builder().http2_prior_knowledge(),
    };
    let clt = clt.build().context("failed to build HTTP client")?;
    let clt = service_fn(|req: Request<OutgoingParams>| async {
        let req = reqwest::Request::try_from(req.map(reqwest::Body::wrap))?;
        let res = clt.execute(req).await?;
        let status = res.status();
        ensure!(status.is_success(), "status `{status}`");
        anyhow::Ok(Response::from(res))
    });
    let uri = Uri::try_from(format!("http://{addr}/"))?;
    let hello = bindings::wrpc_examples::hello::handler::hello(Client::new(uri, clt))
        .await
        .context("failed to invoke `wrpc-examples:hello/handler.hello`")?;
    eprintln!("{hello}");
    Ok(())
}
