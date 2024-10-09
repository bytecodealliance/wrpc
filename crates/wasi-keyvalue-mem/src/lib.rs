use core::num::ParseIntError;

use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::RwLock;
use tracing::{instrument, trace};
use wrpc_transport::{ResourceBorrow, ResourceOwn};
use wrpc_wasi_keyvalue::exports::wasi::keyvalue::{atomics, batch, store};

pub type Result<T, E = store::Error> = core::result::Result<T, E>;

pub type Bucket = Arc<RwLock<HashMap<String, Bytes>>>;

#[derive(Clone, Debug, Default)]
pub struct Handler(pub Arc<RwLock<HashMap<Bytes, Bucket>>>);

impl Handler {
    async fn bucket(&self, bucket: impl AsRef<[u8]>) -> Result<Bucket> {
        trace!("looking up bucket");
        let store = self.0.read().await;
        store
            .get(bucket.as_ref())
            .ok_or(store::Error::NoSuchStore)
            .cloned()
    }
}

impl<C: Send + Sync> store::Handler<C> for Handler {
    // NOTE: Resource handle returned is just the `identifier` itself
    #[instrument(level = "trace", skip(_cx), ret(level = "trace"))]
    async fn open(
        &self,
        _cx: C,
        identifier: String,
    ) -> anyhow::Result<Result<ResourceOwn<store::Bucket>>> {
        let identifier = Bytes::from(identifier);
        {
            // first, optimistically try read-only lock
            let store = self.0.read().await;
            if store.contains_key(&identifier) {
                return Ok(Ok(ResourceOwn::from(identifier)));
            }
        }
        let mut store = self.0.write().await;
        store.entry(identifier.clone()).or_default();
        Ok(Ok(ResourceOwn::from(identifier)))
    }
}

impl<C: Send + Sync> store::HandlerBucket<C> for Handler {
    #[instrument(level = "trace", skip(_cx), ret(level = "trace"))]
    async fn get(
        &self,
        _cx: C,
        bucket: ResourceBorrow<store::Bucket>,
        key: String,
    ) -> anyhow::Result<Result<Option<Bytes>>> {
        let bucket = self.bucket(bucket).await?;
        let bucket = bucket.read().await;
        Ok(Ok(bucket.get(&key).cloned()))
    }

    #[instrument(level = "trace", skip(_cx), ret(level = "trace"))]
    async fn set(
        &self,
        _cx: C,
        bucket: ResourceBorrow<store::Bucket>,
        key: String,
        value: Bytes,
    ) -> anyhow::Result<Result<()>> {
        let bucket = self.bucket(bucket).await?;
        let mut bucket = bucket.write().await;
        bucket.insert(key, value);
        Ok(Ok(()))
    }

    #[instrument(level = "trace", skip(_cx), ret(level = "trace"))]
    async fn delete(
        &self,
        _cx: C,
        bucket: ResourceBorrow<store::Bucket>,
        key: String,
    ) -> anyhow::Result<Result<()>> {
        let bucket = self.bucket(bucket).await?;
        let mut bucket = bucket.write().await;
        bucket.remove(&key);
        Ok(Ok(()))
    }

    #[instrument(level = "trace", skip(_cx), ret(level = "trace"))]
    async fn exists(
        &self,
        _cx: C,
        bucket: ResourceBorrow<store::Bucket>,
        key: String,
    ) -> anyhow::Result<Result<bool>> {
        let bucket = self.bucket(bucket).await?;
        let bucket = bucket.read().await;
        Ok(Ok(bucket.contains_key(&key)))
    }

    #[instrument(level = "trace", skip(_cx), ret(level = "trace"))]
    async fn list_keys(
        &self,
        _cx: C,
        bucket: ResourceBorrow<store::Bucket>,
        cursor: Option<String>,
    ) -> anyhow::Result<Result<store::KeyResponse>> {
        let bucket = self.bucket(bucket).await?;
        let bucket = bucket.read().await;
        let bucket = bucket.keys();
        let keys = if let Some(cursor) = cursor {
            let cursor = cursor
                .parse()
                .map_err(|err: ParseIntError| store::Error::Other(err.to_string()))?;
            bucket.skip(cursor).cloned().collect()
        } else {
            bucket.cloned().collect()
        };
        Ok(Ok(store::KeyResponse { keys, cursor: None }))
    }
}

impl<C: Send + Sync> atomics::Handler<C> for Handler {
    async fn increment(
        &self,
        _cx: C,
        _bucket: ResourceBorrow<store::Bucket>,
        _key: String,
        _delta: i64,
    ) -> anyhow::Result<Result<i64>> {
        Ok(Err(store::Error::Other("not supported".to_string())))
    }

    async fn swap(
        &self,
        _cx: C,
        _cas: ResourceOwn<atomics::Cas>,
        _value: Bytes,
    ) -> anyhow::Result<Result<(), atomics::CasError>> {
        Ok(Err(atomics::CasError::StoreError(store::Error::Other(
            "not supported".to_string(),
        ))))
    }
}

impl<C: Send + Sync> atomics::HandlerCas<C> for Handler {
    async fn new(
        &self,
        _cx: C,
        _bucket: ResourceBorrow<store::Bucket>,
        _key: String,
    ) -> anyhow::Result<Result<ResourceOwn<atomics::Cas>>> {
        Ok(Err(store::Error::Other("not supported".to_string())))
    }

    async fn current(
        &self,
        _cx: C,
        _bucket: ResourceBorrow<atomics::Cas>,
    ) -> anyhow::Result<Result<Option<Bytes>>> {
        Ok(Err(store::Error::Other("not supported".to_string())))
    }
}

impl<C: Send + Sync> batch::Handler<C> for Handler {
    async fn get_many(
        &self,
        _cx: C,
        _bucket: ResourceBorrow<store::Bucket>,
        _keys: Vec<String>,
    ) -> anyhow::Result<Result<Vec<Option<(String, Bytes)>>>> {
        Ok(Err(store::Error::Other("not supported".to_string())))
    }

    async fn set_many(
        &self,
        _cx: C,
        _bucket: ResourceBorrow<store::Bucket>,
        _key_values: Vec<(String, Bytes)>,
    ) -> anyhow::Result<Result<()>> {
        Ok(Err(store::Error::Other("not supported".to_string())))
    }

    async fn delete_many(
        &self,
        _cx: C,
        _bucket: ResourceBorrow<store::Bucket>,
        _keys: Vec<String>,
    ) -> anyhow::Result<Result<()>> {
        Ok(Err(store::Error::Other("not supported".to_string())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(unused)]
    async fn bound(s: &impl wrpc_transport::Serve) -> anyhow::Result<()> {
        wrpc_wasi_keyvalue::serve(s, Handler::default()).await?;
        Ok(())
    }
}
