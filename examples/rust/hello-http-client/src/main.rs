use std::net::SocketAddr;

use anyhow::{Context as _, ensure};
use clap::{Parser, ValueEnum};
use http::Uri;
use hyper_util::client::legacy;
use hyper_util::rt::TokioExecutor;
use tower::ServiceExt as _;
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

    let clt = legacy::Client::builder(TokioExecutor::default())
        .http2_only(matches!(http, Version::Http2))
        .build_http::<OutgoingParams>()
        .map_result(|res| -> anyhow::Result<_> {
            let res = res?;
            let status = res.status();
            ensure!(status.is_success(), "status `{status}`");
            Ok(res)
        });
    let uri = Uri::try_from(format!("http://{addr}/"))?;
    let hello = bindings::wrpc_examples::hello::handler::hello(Client::new(uri, clt))
        .await
        .context("failed to invoke `wrpc-examples:hello/handler.hello`")?;
    eprintln!("{hello}");
    Ok(())
}
