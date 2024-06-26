use core::net::Ipv4Addr;

use core::pin::pin;
use std::sync::Arc;

use anyhow::Context as _;
use futures::StreamExt as _;
use quinn::crypto::rustls::QuicClientConfig;
use quinn::{ClientConfig, EndpointConfig, ServerConfig, TokioRuntime};
use rcgen::{generate_simple_self_signed, CertifiedKey};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use rustls::version::TLS13;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::try_join;
use tracing::info;
use wrpc_transport::{Index as _, Invoke as _, Serve as _};
use wrpc_transport_quic::{Client, Server};

#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn loopback() -> anyhow::Result<()> {
    let CertifiedKey {
        cert: srv_crt,
        key_pair: srv_key,
    } = generate_simple_self_signed(["bar.foo.server.wrpc".into()]).unwrap();
    let CertifiedKey {
        cert: clt_crt,
        key_pair: clt_key,
    } = generate_simple_self_signed(["bar.foo.client.wrpc".into()]).unwrap();
    let srv_crt = CertificateDer::from(srv_crt);

    let mut ca = rustls::RootCertStore::empty();
    ca.add(srv_crt.clone())?;
    let clt_cnf = rustls::ClientConfig::builder_with_protocol_versions(&[&TLS13])
        .with_root_certificates(ca)
        .with_client_auth_cert(
            vec![clt_crt.into()],
            PrivatePkcs8KeyDer::from(clt_key.serialize_der()).into(),
        )
        .context("failed to create client config")?;
    let clt_cnf: QuicClientConfig = clt_cnf
        .try_into()
        .context("failed to convert rustls client config to QUIC client config")?;
    let srv_cnf = ServerConfig::with_single_cert(
        vec![srv_crt],
        PrivatePkcs8KeyDer::from(srv_key.serialize_der()).into(),
    )
    .expect("failed to create server config");

    let mut clt_ep = quinn::Endpoint::client((Ipv4Addr::LOCALHOST, 0).into())
        .context("failed to create client endpoint")?;
    clt_ep.set_default_client_config(ClientConfig::new(Arc::new(clt_cnf)));

    let srv_sock = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
        .context("failed to open a UDP socket")?;
    let srv_addr = srv_sock
        .local_addr()
        .context("failed to query server address")?;
    let srv_ep = quinn::Endpoint::new(
        EndpointConfig::default(),
        Some(srv_cnf),
        srv_sock,
        Arc::new(TokioRuntime),
    )
    .context("failed to create server endpoint")?;

    let clt = Client::new(clt_ep, (Ipv4Addr::LOCALHOST, srv_addr.port()));
    let srv = Server::default();
    let invocations = srv
        .serve("foo", "bar", [[Some(42), Some(0)]])
        .await
        .context("failed to serve `foo.bar`")?;
    let mut invocations = pin!(invocations);
    try_join!(
        async {
            let (mut outgoing, mut incoming) = clt
                .invoke((), "foo", "bar", "test".into(), &[&[Some(0), Some(42)]])
                .await
                .context("failed to invoke `foo.bar`")?;
            let mut nested_tx = outgoing.index(&[42, 0]).context("failed to index `42.0`")?;
            let mut nested_rx = incoming.index(&[0, 42]).context("failed to index `0.42`")?;
            try_join!(
                async {
                    info!("reading `foo`");
                    let mut buf = vec![];
                    let n = incoming
                        .read_to_end(&mut buf)
                        .await
                        .context("failed to read `foo`")?;
                    assert_eq!(n, 3);
                    assert_eq!(buf, b"foo");
                    info!("read `foo`");
                    anyhow::Ok(())
                },
                async {
                    info!("writing `bar`");
                    outgoing
                        .write_all(b"bar")
                        .await
                        .context("failed to write `bar`")?;
                    outgoing
                        .shutdown()
                        .await
                        .context("failed to shutdown stream")?;
                    info!("wrote `bar`");
                    anyhow::Ok(())
                },
                async {
                    info!("writing `client->server`");
                    nested_tx
                        .write_all(b"client->server")
                        .await
                        .context("failed to write `client->server`")?;
                    nested_tx
                        .shutdown()
                        .await
                        .context("failed to shutdown stream")?;
                    info!("wrote `client->server`");
                    anyhow::Ok(())
                },
                async {
                    info!("reading `server->client`");
                    let mut buf = vec![];
                    nested_rx
                        .read_to_end(&mut buf)
                        .await
                        .context("failed to read `server->client`")?;
                    assert_eq!(buf, b"server->client");
                    info!("read `server->client`");
                    anyhow::Ok(())
                },
            )?;
            anyhow::Ok(())
        },
        async {
            let ok = srv
                .accept(&srv_ep)
                .await
                .context("failed to accept client connection")?;
            assert!(ok);
            let ((), mut outgoing, mut incoming) = invocations
                .next()
                .await
                .context("invocation stream unexpectedly finished")?
                .context("failed to get invocation")?;
            let mut nested_tx = outgoing.index(&[0, 42]).context("failed to index `0.42`")?;
            let mut nested_rx = incoming.index(&[42, 0]).context("failed to index `42.0`")?;
            try_join!(
                async {
                    info!("reading `test`");
                    let mut buf = vec![0; 4];
                    incoming
                        .read_exact(&mut buf)
                        .await
                        .context("failed to read `test`")?;
                    assert_eq!(buf, b"test");
                    info!("read `test`");

                    info!("reading `bar`");
                    let mut buf = vec![];
                    let n = incoming
                        .read_to_end(&mut buf)
                        .await
                        .context("failed to read `bar`")?;
                    assert_eq!(n, 3);
                    assert_eq!(buf, b"bar");
                    info!("read `bar`");
                    anyhow::Ok(())
                },
                async {
                    info!("writing `foo`");
                    outgoing
                        .write_all(b"foo")
                        .await
                        .context("failed to write `foo`")?;
                    outgoing
                        .shutdown()
                        .await
                        .context("failed to shutdown stream")?;
                    info!("wrote `foo`");
                    anyhow::Ok(())
                },
                async {
                    info!("writing `server->client`");
                    nested_tx
                        .write_all(b"server->client")
                        .await
                        .context("failed to write `server->client`")?;
                    nested_tx
                        .shutdown()
                        .await
                        .context("failed to shutdown stream")?;
                    info!("wrote `server->client`");
                    anyhow::Ok(())
                },
                async {
                    info!("reading `client->server`");
                    let mut buf = vec![];
                    let n = nested_rx
                        .read_to_end(&mut buf)
                        .await
                        .context("failed to read `client->server`")?;
                    assert_eq!(n, 14);
                    assert_eq!(buf, b"client->server");
                    info!("read `client->server`");
                    anyhow::Ok(())
                },
            )?;
            Ok(())
        }
    )?;
    Ok(())
}
