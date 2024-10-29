use anyhow::{ensure, Context as _};
use bytes::Bytes;
use clap::Parser;
use url::Url;
use wrpc_wasi_keyvalue::wasi::keyvalue::store;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// NATS.io URL to connect to
    #[arg(short, long, default_value = "nats://127.0.0.1:4222")]
    nats: Url,

    /// Prefixes to invoke `wasi:keyvalue/store` functions on
    #[arg(default_value = "rust")]
    prefixes: Vec<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let Args { nats, prefixes } = Args::parse();

    let nats = async_nats::connect_with_options(
        String::from(nats),
        async_nats::ConnectOptions::new().retry_on_initial_connect(),
    )
    .await
    .context("failed to connect to NATS.io server")?;

    for prefix in prefixes {
        let wrpc = wrpc_transport_nats::Client::new(nats.clone(), prefix.clone(), None)
            .await
            .context("failed to construct transport client")?;
        let bucket = store::open(&wrpc, None, "example")
            .await
            .context("failed to invoke `open`")?
            .context("failed to open empty bucket")?;
        store::Bucket::set(&wrpc, None, &bucket.as_borrow(), "foo", &Bytes::from("bar"))
            .await
            .context("failed to invoke `set`")?
            .context("failed to set `foo`")?;
        let ok = store::Bucket::exists(&wrpc, None, &bucket.as_borrow(), "foo")
            .await
            .context("failed to invoke `exists`")?
            .context("failed to check if `foo` exists")?;
        ensure!(ok);
        let v = store::Bucket::get(&wrpc, None, &bucket.as_borrow(), "foo")
            .await
            .context("failed to invoke `get`")?
            .context("failed to get `foo`")?;
        ensure!(v.as_deref() == Some(b"bar".as_slice()));

        let store::KeyResponse { keys, cursor: _ } =
            store::Bucket::list_keys(&wrpc, None, &bucket.as_borrow(), None)
                .await
                .context("failed to invoke `list-keys`")?
                .context("failed to list keys")?;
        for key in keys {
            println!("{prefix} key: {key}");
        }
    }
    Ok(())
}
