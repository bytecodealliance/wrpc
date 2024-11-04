use core::net::Ipv6Addr;

use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;
use quinn::{crypto::rustls::QuicClientConfig, rustls::version::TLS13, ClientConfig, Endpoint};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use url::Url;

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
    let mut ep = Endpoint::client((Ipv6Addr::UNSPECIFIED, 0).into())
        .context("failed to create QUIC endpoint")?;
    let cnf = rustls::ClientConfig::builder_with_protocol_versions(&[&TLS13])
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Insecure))
        .with_no_client_auth();
    let cnf: QuicClientConfig = cnf
        .try_into()
        .context("failed to convert rustls client config to QUIC client config")?;
    ep.set_default_client_config(ClientConfig::new(Arc::new(cnf)));
    let session = web_transport_quinn::connect(&ep, &addr)
        .await
        .context("failed to connect to server")?;
    let wrpc = wrpc_transport_web::Client::from(session);
    let hello = bindings::wrpc_examples::hello::handler::hello(&wrpc, ())
        .await
        .context("failed to invoke `wrpc-examples.hello/handler.hello`")?;
    eprintln!("{hello}");
    Ok(())
}
