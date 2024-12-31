use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::net::TcpListener;
use tracing::{error, info};

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
    /// Address to serve `wrpc-examples:hello/handler.hello` on
    #[arg(default_value = "[::1]:8080")]
    ingress: String,

    /// Address to invoke `wrpc-examples:hello/handler.hello` on
    #[arg(default_value = "[::1]:7761")]
    egress: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let Args { egress, ingress } = Args::parse();
    let wrpc = wrpc_transport::tcp::Client::from(egress);
    let wrpc = Arc::new(wrpc);

    let svc = hyper::service::service_fn(move |req| {
        let wrpc = Arc::clone(&wrpc);
        async move {
            let (http::request::Parts { method, uri, .. }, _) = req.into_parts();
            match (method.as_str(), uri.path_and_query().map(|pq| pq.as_str())) {
                ("GET", Some("/hello")) => {
                    match bindings::wrpc_examples::hello::handler::hello(wrpc.as_ref(), ())
                        .await
                        .context("failed to invoke `wrpc-examples.hello/handler.hello`")
                    {
                        Ok(hello) => Ok(http::Response::new(format!(r#""{hello}""#))),
                        Err(err) => Err(format!("{err:#}")),
                    }
                }
                (method, Some(path)) => {
                    Err(format!("method `{method}` not supported for path `{path}`"))
                }
                (method, None) => Err(format!("method `{method}` not supported")),
            }
        }
    });
    let srv = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
    let socket = TcpListener::bind(&ingress)
        .await
        .with_context(|| format!("failed to bind on {ingress}"))?;
    loop {
        let stream = match socket.accept().await {
            Ok((stream, addr)) => {
                info!(?addr, "accepted HTTP connection");
                stream
            }
            Err(err) => {
                error!(?err, "failed to accept HTTP endpoint connection");
                continue;
            }
        };
        let svc = svc.clone();
        if let Err(err) = srv.serve_connection(TokioIo::new(stream), svc).await {
            error!(?err, "failed to serve HTTP endpoint connection");
        }
    }
}
