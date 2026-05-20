use core::net::SocketAddr;
use core::pin::{pin, Pin};

use std::sync::Arc;

use anyhow::Context as _;
use bytes::Bytes;
use clap::Parser;
use futures::stream::select_all;
use futures::{Stream, StreamExt as _};
use tokio::task::JoinSet;
use tokio::{select, signal};
use tracing::{debug, error, info, warn};

mod bindings {
    wit_bindgen_wrpc::generate!({
        with: {
            "wrpc-examples:streams/handler": generate,
        }
    });
}

use bindings::exports::wrpc_examples::streams::handler::Req;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Address to serve `wrpc-examples:streams/handler.echo` on
    #[arg(default_value = "[::1]:7762")]
    addr: String,
}

#[derive(Clone, Copy)]
struct Server;

impl bindings::exports::wrpc_examples::streams::handler::Handler<SocketAddr> for Server {
    async fn echo(
        &self,
        _ctx: SocketAddr,
        Req { numbers, bytes }: Req,
    ) -> anyhow::Result<(
        Pin<Box<dyn Stream<Item = Vec<u64>> + Send>>,
        Pin<Box<dyn Stream<Item = Bytes> + Send>>,
    )> {
        Ok((numbers, bytes))
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let Args { addr } = Args::parse();

    let lis = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("failed to bind TCP listener on `{addr}`"))?;
    let srv = Arc::new(wrpc_transport::Server::default());
    let accept = tokio::spawn({
        let srv = Arc::clone(&srv);
        async move {
            loop {
                if let Err(err) = srv.accept(&lis).await {
                    error!(?err, "failed to accept TCP connection");
                }
            }
        }
    });

    let invocations = bindings::serve(srv.as_ref(), Server)
        .await
        .context("failed to serve `wrpc-examples:streams/handler.echo`")?;
    // NOTE: This will conflate all invocation streams into a single stream via `futures::stream::SelectAll`,
    // to customize this, iterate over the returned `invocations` and set up custom handling per export
    let mut invocations = select_all(
        invocations
            .into_iter()
            .map(|(instance, name, invocations)| invocations.map(move |res| (instance, name, res))),
    );
    let shutdown = signal::ctrl_c();
    let mut shutdown = pin!(shutdown);
    let mut tasks = JoinSet::new();
    loop {
        select! {
            Some((instance, name, res)) = invocations.next() => {
                match res {
                    Ok(fut) => {
                        debug!(instance, name, "invocation accepted");
                        tasks.spawn(async move {
                            if let Err(err) = fut.await {
                                warn!(?err, "failed to handle invocation");
                            } else {
                                info!(instance, name, "invocation successfully handled");
                            }
                        });
                    }
                    Err(err) => {
                        warn!(?err, instance, name, "failed to accept invocation");
                    }
                }
            }
            Some(res) = tasks.join_next() => {
                if let Err(err) = res {
                    error!(?err, "failed to join task")
                }
            }
            res = &mut shutdown => {
                accept.abort();
                // wait for all invocations to complete
                while let Some(res) = tasks.join_next().await {
                    if let Err(err) = res {
                        error!(?err, "failed to join task")
                    }
                }
                return res.context("failed to listen for ^C")
            }
        }
    }
}
