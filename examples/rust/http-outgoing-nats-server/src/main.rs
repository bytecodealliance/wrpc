use core::pin::pin;

use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;
use futures::stream::StreamExt as _;
use http::uri::Uri;
use hyper_util::rt::TokioExecutor;
use tokio::{select, signal, spawn, sync::mpsc};
use tracing::{debug, error, instrument, warn};
use wrpc_interface_http::{
    try_http_to_outgoing_response, IncomingBody, IncomingFields, IncomingRequestHttp,
    RequestOptions,
};
use wrpc_transport::{AcceptedInvocation, IncomingInputStream, Transmitter};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// NATS.io URL to connect to
    #[arg(short, long, default_value = "nats://127.0.0.1:4222")]
    nats: Uri,

    /// Prefix to serve `wrpc:http/ougoing-handler.handle` on
    #[arg(default_value = "rust")]
    prefix: String,
}

#[instrument(level = "trace", skip_all)]
async fn serve_handle<Tx: Transmitter>(
    client: &hyper_util::client::legacy::Client<
        hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
        IncomingBody<IncomingInputStream, IncomingFields>,
    >,
    AcceptedInvocation {
        params: (IncomingRequestHttp(req), opts),
        result_subject,
        transmitter,
        ..
    }: AcceptedInvocation<
        Option<async_nats::HeaderMap>,
        (IncomingRequestHttp, Option<RequestOptions>),
        Tx,
    >,
) {
    // TODO: Use opts
    let _ = opts;
    debug!(uri = ?req.uri(), "send HTTP request");
    let res = match client.request(req).await.map(try_http_to_outgoing_response) {
        Ok(Ok((res, errors))) => {
            debug!("received HTTP response");
            // TODO: Handle body errors
            spawn(errors.for_each(|err| async move { error!(?err, "body error encountered") }));
            Ok(res)
        }
        Ok(Err(err)) => {
            error!(
                ?err,
                "failed to convert `http` response to `wrpc:http` response"
            );
            return;
        }
        Err(err) => {
            debug!(?err, "failed to send HTTP request");
            Err(wrpc_interface_http::ErrorCode::InternalError(Some(
                err.to_string(),
            )))
        }
    };
    if let Err(err) = transmitter.transmit_static(result_subject, res).await {
        error!(?err, "failed to transmit response");
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let Args { nats, prefix } = Args::parse();

    let native_roots = match rustls_native_certs::load_native_certs() {
        Ok(certs) => certs.into(),
        Err(err) => {
            warn!(?err, "failed to load native root certificate store");
            vec![]
        }
    };
    let mut ca = rustls::RootCertStore::empty();
    let (added, ignored) = ca.add_parsable_certificates(native_roots);
    debug!(added, ignored, "loaded native root certificate store");
    ca.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let client = hyper_util::client::legacy::Client::builder(TokioExecutor::new()).build(
        hyper_rustls::HttpsConnectorBuilder::new()
            .with_tls_config(
                rustls::ClientConfig::builder()
                    .with_root_certificates(ca)
                    .with_no_client_auth(),
            )
            .https_or_http()
            .enable_all_versions()
            .build(),
    );

    let nats = connect(nats)
        .await
        .context("failed to connect to NATS.io")?;
    let wrpc = wrpc_transport_nats::Client::new(nats.clone(), prefix);

    let client = Arc::new(client);
    let mut shutdown = pin!(async { signal::ctrl_c().await.expect("failed to listen for ^C") });
    'outer: loop {
        use wrpc_interface_http::OutgoingHandler as _;
        let handle_invocations = wrpc
            .serve_handle_http()
            .await
            .context("failed to serve `wrpc:http/outgoing-handler.handle` invocations")?;
        let mut handle_invocations = pin!(handle_invocations);
        loop {
            select! {
                invocation = handle_invocations.next() => {
                    match invocation {
                        Some(Ok(invocation)) => {
                            let client = Arc::clone(&client);
                            spawn(async move { serve_handle(client.as_ref(), invocation).await });
                        },
                        Some(Err(err)) => {
                            error!(?err, "failed to accept `wrpc:http/outgoing-handler.handle` invocation");
                        },
                        None => {
                            warn!("`wrpc:http/outgoing-handler.handle` stream unexpectedly finished, resubscribe");
                            continue 'outer
                        }
                    }
                }
                () = &mut shutdown => {
                    debug!("^C received, exit");
                    return Ok(())
                }
            }
        }
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
