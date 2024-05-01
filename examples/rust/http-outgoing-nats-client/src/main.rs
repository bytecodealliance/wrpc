use core::str;

use anyhow::{bail, Context as _};
use clap::Parser;
use futures::stream::{self, TryStreamExt as _};
use http::uri::Uri;
use tokio::io::{stdout, AsyncWriteExt as _};
use tokio::sync::mpsc;
use tokio::try_join;
use tracing::debug;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use wrpc_interface_http::{OutgoingHandler as _, Request, Response};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// NATS.io URL to connect to
    #[arg(short, long, default_value = "nats://127.0.0.1:4222")]
    nats: Uri,

    /// Prefix to send HTTP request on
    #[arg(short, long)]
    prefix: String,

    /// HTTP method
    #[arg(short, long, default_value_t = http::Method::GET)]
    method: http::Method,

    /// URL to send request to
    url: Uri,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().compact().without_time())
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let Args {
        nats,
        prefix,
        method,
        url,
    } = Args::parse();

    let nats = connect(nats)
        .await
        .context("failed to connect to NATS.io")?;
    let wrpc = wrpc_transport_nats::Client::new(nats.clone(), prefix);
    let path = url.path();
    let (res, tx) = wrpc
        .invoke_handle(
            Request {
                body: stream::empty(),
                trailers: async { None },
                method: (&method).into(),
                path_with_query: Some(if let Some(query) = url.query() {
                    format!("{path}?{query}")
                } else {
                    path.to_string()
                }),
                scheme: url.scheme().map(Into::into),
                authority: url.authority().map(ToString::to_string),
                headers: vec![],
            },
            None,
        )
        .await
        .context("failed to invoke `wrpc:http/outgoing-handler.handle`")?;
    match res {
        Ok(Response {
            body,
            trailers,
            status,
            headers,
        }) => {
            println!("{status}");
            println!();

            for (header, values) in headers {
                for value in values {
                    let value = str::from_utf8(&value)
                        .with_context(|| format!("`{header}` header value is not valid UTF-8"))?;
                    println!("{header}: {value}");
                }
            }
            println!();

            try_join!(
                async { tx.await.context("failed to transmit request") },
                async {
                    debug!("receiving body");
                    body.try_for_each(|chunk| async move {
                        stdout()
                            .write_all(&chunk)
                            .await
                            .context("failed to write body chunk to STDOUT")?;
                        Ok(())
                    })
                    .await
                    .context("failed to receive response body")?;
                    debug!("received body");

                    println!();

                    debug!("receiving trailers");
                    let trailers = trailers.await.context("failed to receive trailers")?;
                    if let Some(trailers) = trailers {
                        for (trailer, values) in trailers {
                            for value in values {
                                let value = str::from_utf8(&value).with_context(|| {
                                    format!("`{trailer}` trailer value is not valid UTF-8")
                                })?;
                                println!("{trailer}: {value}");
                            }
                        }
                    }
                    debug!("received trailers");
                    Ok(())
                }
            )?;
            Ok(())
        }
        Err(code) => bail!("request failed with code: {code:?}"),
    }
}

/// Connect to NATS.io server and ensure that the connection is fully established before
/// returning the resulting [`async_nats::Client`]
async fn connect(url: Uri) -> anyhow::Result<async_nats::Client> {
    let (conn_tx, mut conn_rx) = mpsc::channel(1);
    let client = async_nats::connect_with_options(
        url.to_string(),
        async_nats::ConnectOptions::new()
            .retry_on_initial_connect()
            .event_callback(move |event| {
                let conn_tx = conn_tx.clone();
                async move {
                    if let async_nats::Event::Connected = event {
                        conn_tx
                            .send(())
                            .await
                            .expect("failed to send NATS.io server connection notification");
                    }
                }
            }),
    )
    .await
    .context("failed to connect to NATS.io server")?;
    conn_rx
        .recv()
        .await
        .context("failed to await NATS.io server connection to be established")?;
    Ok(client)
}
