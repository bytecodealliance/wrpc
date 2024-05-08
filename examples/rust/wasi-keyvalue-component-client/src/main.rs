#[allow(clippy::missing_safety_doc)]
mod bindings {
    wit_bindgen::generate!();
}

use anyhow::{ensure, Context as _};

use bindings::wasi::keyvalue::store;

fn main() -> anyhow::Result<()> {
    let bucket = store::open("example").context("failed to open empty bucket")?;
    bucket.set("foo", b"bar").context("failed to set `foo`")?;
    let ok = bucket
        .exists("foo")
        .context("failed to check if `foo` exists")?;
    ensure!(ok);
    let v = bucket.get("foo").context("failed to get `foo`")?;
    ensure!(v.as_deref() == Some(b"bar".as_slice()));

    let store::KeyResponse { keys, cursor: _ } =
        bucket.list_keys(None).context("failed to list keys")?;
    for key in keys {
        println!("key: {key}");
    }
    Ok(())
}
