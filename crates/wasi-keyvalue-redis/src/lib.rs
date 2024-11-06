use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context as _;
use bytes::Bytes;
use redis::{aio::ConnectionManager, AsyncCommands as _};
use tokio::sync::RwLock;
use tracing::{instrument, trace};
use uuid::Uuid;
use wrpc_transport::{ResourceBorrow, ResourceOwn};
use wrpc_wasi_keyvalue::exports::wasi::keyvalue::{atomics, batch, store};

pub type Result<T, E = store::Error> = core::result::Result<T, E>;

#[derive(Clone, Default)]
pub struct Handler(pub Arc<RwLock<HashMap<Bytes, ConnectionManager>>>);

impl Handler {
    async fn bucket(&self, bucket: impl AsRef<[u8]>) -> Result<ConnectionManager> {
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
    #[instrument(level = "trace", skip(self, _cx), ret(level = "trace"))]
    async fn open(
        &self,
        _cx: C,
        identifier: String,
    ) -> anyhow::Result<Result<ResourceOwn<store::Bucket>>> {
        let client = match redis::Client::open(identifier).context("failed to open Redis client") {
            Ok(client) => client,
            Err(err) => return Ok(Err(store::Error::Other(format!("{err:#}")))),
        };
        let conn = match client
            .get_connection_manager()
            .await
            .context("failed to get Redis connection manager")
        {
            Ok(conn) => conn,
            Err(err) => return Ok(Err(store::Error::Other(format!("{err:#}")))),
        };
        let id = Uuid::now_v7();
        let id = Bytes::copy_from_slice(id.as_bytes());
        let mut buckets = self.0.write().await;
        buckets.insert(id.clone(), conn);
        return Ok(Ok(ResourceOwn::from(id)));
    }
}

impl<C: Send + Sync> store::HandlerBucket<C> for Handler {
    #[instrument(level = "trace", skip(self, _cx), ret(level = "trace"))]
    async fn get(
        &self,
        _cx: C,
        bucket: ResourceBorrow<store::Bucket>,
        key: String,
    ) -> anyhow::Result<Result<Option<Bytes>>> {
        let mut conn = match self.bucket(bucket).await {
            Ok(conn) => conn,
            Err(err) => return Ok(Err(err)),
        };
        match conn.get(key).await {
            Ok(redis::Value::Nil) => Ok(Ok(None)),
            Ok(redis::Value::BulkString(buf)) => Ok(Ok(Some(buf.into()))),
            Ok(_) => Ok(Err(store::Error::Other(
                "invalid data type returned by Redis".into(),
            ))),
            Err(err) => Ok(Err(store::Error::Other(err.to_string()))),
        }
    }

    #[instrument(level = "trace", skip(self, _cx), ret(level = "trace"))]
    async fn set(
        &self,
        _cx: C,
        bucket: ResourceBorrow<store::Bucket>,
        key: String,
        value: Bytes,
    ) -> anyhow::Result<Result<()>> {
        let mut conn = match self.bucket(bucket).await {
            Ok(conn) => conn,
            Err(err) => return Ok(Err(err)),
        };
        match conn.set(key, value.as_ref()).await {
            Ok(()) => Ok(Ok(())),
            Err(err) => Ok(Err(store::Error::Other(err.to_string()))),
        }
    }

    #[instrument(level = "trace", skip(self, _cx), ret(level = "trace"))]
    async fn delete(
        &self,
        _cx: C,
        bucket: ResourceBorrow<store::Bucket>,
        key: String,
    ) -> anyhow::Result<Result<()>> {
        let mut conn = match self.bucket(bucket).await {
            Ok(conn) => conn,
            Err(err) => return Ok(Err(err)),
        };
        match conn.del(key).await {
            Ok(()) => Ok(Ok(())),
            Err(err) => Ok(Err(store::Error::Other(err.to_string()))),
        }
    }

    #[instrument(level = "trace", skip(self, _cx), ret(level = "trace"))]
    async fn exists(
        &self,
        _cx: C,
        bucket: ResourceBorrow<store::Bucket>,
        key: String,
    ) -> anyhow::Result<Result<bool>> {
        let mut conn = match self.bucket(bucket).await {
            Ok(conn) => conn,
            Err(err) => return Ok(Err(err)),
        };
        match conn.exists(key).await {
            Ok(ok) => Ok(Ok(ok)),
            Err(err) => Ok(Err(store::Error::Other(err.to_string()))),
        }
    }

    #[instrument(level = "trace", skip(self, _cx), ret(level = "trace"))]
    async fn list_keys(
        &self,
        _cx: C,
        bucket: ResourceBorrow<store::Bucket>,
        cursor: Option<String>,
    ) -> anyhow::Result<Result<store::KeyResponse>> {
        Ok(Err(store::Error::Other("not supported".into())))
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
        Ok(Err(store::Error::Other("not supported".into())))
    }

    async fn swap(
        &self,
        _cx: C,
        _cas: ResourceOwn<atomics::Cas>,
        _value: Bytes,
    ) -> anyhow::Result<Result<(), atomics::CasError>> {
        Ok(Err(atomics::CasError::StoreError(store::Error::Other(
            "not supported".into(),
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
        Ok(Err(store::Error::Other("not supported".into())))
    }

    async fn current(
        &self,
        _cx: C,
        _bucket: ResourceBorrow<atomics::Cas>,
    ) -> anyhow::Result<Result<Option<Bytes>>> {
        Ok(Err(store::Error::Other("not supported".into())))
    }
}

impl<C: Send + Sync> batch::Handler<C> for Handler {
    async fn get_many(
        &self,
        _cx: C,
        _bucket: ResourceBorrow<store::Bucket>,
        _keys: Vec<String>,
    ) -> anyhow::Result<Result<Vec<Option<(String, Bytes)>>>> {
        Ok(Err(store::Error::Other("not supported".into())))
    }

    async fn set_many(
        &self,
        _cx: C,
        _bucket: ResourceBorrow<store::Bucket>,
        _key_values: Vec<(String, Bytes)>,
    ) -> anyhow::Result<Result<()>> {
        Ok(Err(store::Error::Other("not supported".into())))
    }

    async fn delete_many(
        &self,
        _cx: C,
        _bucket: ResourceBorrow<store::Bucket>,
        _keys: Vec<String>,
    ) -> anyhow::Result<Result<()>> {
        Ok(Err(store::Error::Other("not supported".into())))
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
