use std::sync::Arc;

use anyhow::{ensure, Context as _};
use bytes::Bytes;
use clap::Parser;
use core::net::SocketAddr;
use quinn::Endpoint;
use quinn::{crypto::rustls::QuicClientConfig, ClientConfig};
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, ServerName, UnixTime},
    version::TLS13,
    DigitallySignedStruct, SignatureScheme,
};
use wrpc_wasi_keyvalue::wasi::keyvalue::store;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Address to invoke `wrpc-examples:hello/handler.hello` on
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
    let bucket = store::open(&wrpc, (), "example")
        .await
        .context("failed to invoke `open`")?
        .context("failed to open empty bucket")?;
    store::Bucket::set(&wrpc, (), &bucket.as_borrow(), "foo", &Bytes::from("bar"))
        .await
        .context("failed to invoke `set`")?
        .context("failed to set `foo`")?;
    let ok = store::Bucket::exists(&wrpc, (), &bucket.as_borrow(), "foo")
        .await
        .context("failed to invoke `exists`")?
        .context("failed to check if `foo` exists")?;
    ensure!(ok);
    let v = store::Bucket::get(&wrpc, (), &bucket.as_borrow(), "foo")
        .await
        .context("failed to invoke `get`")?
        .context("failed to get `foo`")?;
    ensure!(v.as_deref() == Some(b"bar".as_slice()));

    let store::KeyResponse { keys, cursor: _ } =
        store::Bucket::list_keys(&wrpc, (), &bucket.as_borrow(), None)
            .await
            .context("failed to invoke `list-keys`")?
            .context("failed to list keys")?;
    for key in keys {
        println!("key: {key}");
    }

    Ok(())
}
