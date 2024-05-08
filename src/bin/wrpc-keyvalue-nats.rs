use anyhow::Context as _;
use clap::{Parser, Subcommand};
use tokio::io::{stdin, stdout, AsyncReadExt as _, AsyncWriteExt as _};
use url::Url;

mod bindings {
    wit_bindgen_wrpc::generate!({
        world: "keyvalue-client",
        additional_derives: [serde::Serialize],
    });
}

#[derive(Debug, Subcommand)]
#[allow(clippy::enum_variant_names)]
enum Operation {
    #[clap(name = "store.get")]
    StoreGet { bucket: String, key: String },
    #[clap(name = "store.set")]
    StoreSet { bucket: String, key: String },
    #[clap(name = "store.delete")]
    StoreDelete { bucket: String, key: String },
    #[clap(name = "store.exists")]
    StoreExists { bucket: String, key: String },
    #[clap(name = "store.list-keys")]
    StoreListKeys { bucket: String, cursor: Option<u64> },
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// NATS.io URL to connect to
    #[arg(short, long, default_value = wrpc_cli::nats::DEFAULT_URL)]
    nats: Url,

    /// Prefix to invoke `wrpc:keyvalue` operations on
    prefix: String,

    /// `wrpc:keyvalue` function to invoke
    #[clap(subcommand)]
    operation: Operation,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    wrpc_cli::tracing::init();

    let Args {
        nats,
        prefix,
        operation,
    } = Args::parse();

    let nats = wrpc_cli::nats::connect(nats)
        .await
        .context("failed to connect to NATS.io")?;
    let wrpc = wrpc_transport_nats::Client::new(nats, prefix);
    match operation {
        Operation::StoreGet { bucket, key } => {
            if let Some(v) = bindings::wrpc::keyvalue::store::get(&wrpc, &bucket, &key)
                .await
                .context("failed to invoke `wrpc:keyvalue/store.get`")?
                .context("`wrpc:keyvalue/store.get` failed")?
            {
                stdout()
                    .write_all(&v)
                    .await
                    .context("failed to write response to stdout")?;
            }
        }
        Operation::StoreSet { bucket, key } => {
            let mut buf = vec![];
            stdin()
                .read_to_end(&mut buf)
                .await
                .context("failed to read from stdin")?;
            bindings::wrpc::keyvalue::store::set(&wrpc, &bucket, &key, &buf)
                .await
                .context("failed to invoke `wrpc:keyvalue/store.set`")?
                .context("`wrpc:keyvalue/store.set` failed")?
        }
        Operation::StoreDelete { bucket, key } => {
            bindings::wrpc::keyvalue::store::delete(&wrpc, &bucket, &key)
                .await
                .context("failed to invoke `wrpc:keyvalue/store.delete`")?
                .context("`wrpc:keyvalue/store.delete` failed")?
        }
        Operation::StoreExists { bucket, key } => {
            let ok = bindings::wrpc::keyvalue::store::exists(&wrpc, &bucket, &key)
                .await
                .context("failed to invoke `wrpc:keyvalue/store.exists`")?
                .context("`wrpc:keyvalue/store.exists` failed")?;
            if !ok {
                println!("false")
            } else {
                println!("true")
            }
        }
        Operation::StoreListKeys { bucket, cursor } => {
            let keys = bindings::wrpc::keyvalue::store::list_keys(&wrpc, &bucket, cursor)
                .await
                .context("failed to invoke `wrpc:keyvalue/store.list-keys`")?
                .context("`wrpc:keyvalue/store.list-keys` failed")?;
            serde_json::to_writer_pretty(std::io::stdout(), &keys)
                .context("failed to write response to stdout")?;
        }
    }
    Ok(())
}
