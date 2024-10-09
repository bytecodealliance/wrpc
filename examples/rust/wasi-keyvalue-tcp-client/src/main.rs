use anyhow::{ensure, Context as _};
use bytes::Bytes;
use clap::Parser;
use wrpc_wasi_keyvalue::wasi::keyvalue::store;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Address to invoke `wasi:keyvalue/store` functions on
    #[arg(default_value = "[::1]:7761")]
    addr: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let Args { addr } = Args::parse();

    let wrpc = wrpc_transport::tcp::Client::from(addr);
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
