use anyhow::{Context as _, ensure};
use bytes::Bytes;
use clap::Parser;
use tokio::net::TcpStream;
use wrpc_core::Framed;
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

    let connect = async || {
        let stream = TcpStream::connect(&addr)
            .await
            .with_context(|| format!("failed to connect to `{addr}`"))?;
        stream
            .set_nodelay(true)
            .context("failed to set TCP_NODELAY")?;
        let (rx, tx) = stream.into_split();
        anyhow::Ok(Framed::new(tx, rx))
    };
    let clt = connect().await?;
    let bucket = store::open(clt, "example")
        .await
        .context("failed to invoke `open`")?
        .context("failed to open empty bucket")?;
    let clt = connect().await?;
    store::Bucket::set(clt, &bucket.as_borrow(), "foo", &Bytes::from("bar"))
        .await
        .context("failed to invoke `set`")?
        .context("failed to set `foo`")?;
    let clt = connect().await?;
    let ok = store::Bucket::exists(clt, &bucket.as_borrow(), "foo")
        .await
        .context("failed to invoke `exists`")?
        .context("failed to check if `foo` exists")?;
    ensure!(ok);
    let clt = connect().await?;
    let v = store::Bucket::get(clt, &bucket.as_borrow(), "foo")
        .await
        .context("failed to invoke `get`")?
        .context("failed to get `foo`")?;
    ensure!(v.as_deref() == Some(b"bar".as_slice()));

    let clt = connect().await?;
    let store::KeyResponse { keys, cursor: _ } =
        store::Bucket::list_keys(clt, &bucket.as_borrow(), None)
            .await
            .context("failed to invoke `list-keys`")?
            .context("failed to list keys")?;
    for key in keys {
        println!("key: {key}");
    }
    Ok(())
}
