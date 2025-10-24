mod client_config;

use client_config::configure_client;

use anyhow::{ensure, Context as _};
use bytes::Bytes;
use core::net::SocketAddr;
use quinn::Endpoint;
use std::net::{IpAddr, Ipv4Addr};
use wrpc_wasi_keyvalue::wasi::keyvalue::store;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let client_config = configure_client().context("failed tls verification")?;
    const SERVER_NAME: &str = "localhost";
    const LOCALHOST_V4: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
    const CLIENT_ADDR: SocketAddr = SocketAddr::new(LOCALHOST_V4, 5000);
    const SERVER_ADDR: SocketAddr = SocketAddr::new(LOCALHOST_V4, 4433);
    // Bind this endpoint to a UDP socket on the given client address.
    let endpoint = Endpoint::client(CLIENT_ADDR)?;

    // Connect to the server passing in the server name which is supposed to be in the server certificate.
    let connection = endpoint
        .connect_with(client_config, SERVER_ADDR, SERVER_NAME)?
        .await?;
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
