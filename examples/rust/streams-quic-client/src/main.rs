use core::time::Duration;
use std::sync::Arc;

use anyhow::Context as _;
use bytes::Bytes;
use clap::Parser;
use core::net::SocketAddr;
use futures::{stream, StreamExt as _};
use quinn::{crypto::rustls::QuicClientConfig, ClientConfig, Endpoint};
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, ServerName, UnixTime},
    version::TLS13,
    DigitallySignedStruct, SignatureScheme,
};
use tokio::{time, try_join};
use tokio_stream::wrappers::IntervalStream;
use tracing::debug;

mod bindings {
    wit_bindgen_wrpc::generate!({
        with: {
            "wrpc-examples:streams/handler": generate
        }
    });
}

use bindings::wrpc_examples::streams::handler::{echo, Req};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Address to invoke `wrpc-examples:streams/handler.echo` on
    #[arg(default_value = "127.0.0.1:4433")]
    addr: SocketAddr,
}

#[derive(Debug)]
struct Insecure;

impl ServerCertVerifier for Insecure {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ECDSA_NISTP256_SHA256]
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let Args { addr } = Args::parse();

    let conf = rustls::ClientConfig::builder_with_protocol_versions(&[&TLS13])
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Insecure))
        .with_no_client_auth();

    let conf: ClientConfig = ClientConfig::new(Arc::new(QuicClientConfig::try_from(conf)?));
    // Bind to any IPv4 or IPv6 address (dual stack, if supported).
    let ep = Endpoint::client((std::net::Ipv6Addr::UNSPECIFIED, 0).into())?;

    // Connect using the rustls client configuration, addr, and server name
    let connection = ep
        .connect_with(conf, addr, "localhost")?
        .await
        .context("failed to connect to server")?;
    let wrpc = wrpc_transport_quic::Client::from(connection);

    let numbers = Box::pin(
        stream::iter(1..)
            .take(10)
            .zip(IntervalStream::new(time::interval(Duration::from_secs(1))))
            .map(|(i, _)| i)
            .ready_chunks(10),
    );

    // `stream<u8>` items are chunked using [`Bytes`]
    let bytes = Box::pin(
        stream::iter(b"foo bar baz")
            .zip(IntervalStream::new(time::interval(Duration::from_secs(1))))
            .map(|(i, _)| *i)
            .ready_chunks(10)
            .map(Bytes::from),
    );

    let (mut numbers, mut bytes, io) = echo(&wrpc, (), Req { numbers, bytes })
        .await
        .context("failed to invoke `wrpc-examples:streams/handler.echo`")?;
    try_join!(
        async {
            if let Some(io) = io {
                debug!("performing async I/O");
                io.await.context("failed to complete async I/O")
            } else {
                Ok(())
            }
        },
        async {
            while let Some(item) = numbers.next().await {
                eprintln!("numbers: {item:?}");
            }
            Ok(())
        },
        async {
            while let Some(item) = bytes.next().await {
                eprintln!("bytes: {item:?}");
            }
            Ok(())
        }
    )?;
    Ok(())
}
