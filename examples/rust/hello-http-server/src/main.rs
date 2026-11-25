use std::net::SocketAddr;

use anyhow::Context as _;
use axum::Router;
use axum::body::Body;
use axum::http::StatusCode;
use axum::response::{IntoResponse as _, Response};
use axum::routing::post;
use clap::Parser;
use http_body_util::BodyExt as _;
use tokio::net::TcpListener;
use tokio::signal;
use tracing::{error, warn};
use wrpc::{Header, ServeError};

mod bindings {
    wrpc::generate!({
        with: {
            "wrpc-examples:hello/handler": generate,
        }
    });
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Address to serve `wrpc-examples:hello/handler.hello` on, using HTTP/1.1 and HTTP/2
    #[arg(default_value = "[::1]:7763")]
    addr: SocketAddr,
}

#[derive(Clone, Copy)]
struct Handler;

impl bindings::exports::wrpc_examples::hello::handler::Handler for Handler {
    async fn hello(&self) -> std::io::Result<String> {
        Ok("hello from Rust".to_string())
    }
}

async fn serve(body: Body) -> Response {
    let (Header { instance, name }, req) = match wrpc::http::accept(body).await {
        Ok(req) => req,
        Err(err) => {
            warn!(?err, "failed to accept invocation");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };
    let Ok(fut) = bindings::handle(&Handler, &instance, &name, req) else {
        warn!(instance, name, "function is not exported");
        return StatusCode::NOT_FOUND.into_response();
    };
    match fut.await {
        Ok(res) => res.map(|body| {
            Body::new(body.map_err(|err| {
                match err {
                    ServeError::Request(..) => warn!(?err, "failed to serve invocation"),
                    ServeError::Unread | ServeError::Handler(..) | ServeError::Response(..) => {
                        error!(?err, "failed to serve invocation");
                    }
                }
                err
            }))
        }),
        Err(err @ ServeError::Request(..)) => {
            warn!(?err, "failed to serve invocation");
            StatusCode::BAD_REQUEST.into_response()
        }
        Err(err @ (ServeError::Unread | ServeError::Handler(..) | ServeError::Response(..))) => {
            error!(?err, "failed to serve invocation");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let Args { addr } = Args::parse();
    let lis = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind TCP listener on `{addr}`"))?;
    axum::serve(lis, Router::new().route("/", post(serve)))
        .with_graceful_shutdown(async {
            if let Err(err) = signal::ctrl_c().await {
                error!(?err, "failed to listen for ^C");
            }
        })
        .await
        .context("failed to serve")
}
