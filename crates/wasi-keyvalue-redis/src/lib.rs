use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context as _;
use bytes::Bytes;
use redis::{AsyncCommands as _, aio::ConnectionManager};
use tokio::sync::RwLock;
use tracing::{instrument, trace};
use uuid::Uuid;
use wrpc_transport::{ResourceBorrow, ResourceOwn};
use wrpc_wasi_keyvalue::exports::wasi::keyvalue::{atomics, batch, store};

#[derive(Clone, Default)]
pub struct Handler(pub Arc<RwLock<HashMap<Bytes, ConnectionManager>>>);

impl Handler {
    async fn bucket(&self, bucket: impl AsRef<[u8]>) -> Result<ConnectionManager, store::Error> {
        trace!("looking up bucket");
        let store = self.0.read().await;
        store
            .get(bucket.as_ref())
            .ok_or(store::Error::NoSuchStore)
            .cloned()
    }
}

impl store::Handler for Handler {
    // NOTE: Resource handle returned is just the `identifier` itself
    #[instrument(level = "trace", skip(self), ret(level = "trace"))]
    async fn open(
        &self,
        identifier: String,
    ) -> anyhow::Result<Result<ResourceOwn<store::Bucket>, store::Error>> {
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

impl store::HandlerBucket for Handler {
    #[instrument(level = "trace", skip(self), ret(level = "trace"))]
    async fn get(
        &self,
        bucket: ResourceBorrow<store::Bucket>,
        key: String,
    ) -> anyhow::Result<Result<Option<Bytes>, store::Error>> {
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

    #[instrument(level = "trace", skip(self), ret(level = "trace"))]
    async fn set(
        &self,
        bucket: ResourceBorrow<store::Bucket>,
        key: String,
        value: Bytes,
    ) -> anyhow::Result<Result<(), store::Error>> {
        let mut conn = match self.bucket(bucket).await {
            Ok(conn) => conn,
            Err(err) => return Ok(Err(err)),
        };
        match conn.set(key, value.as_ref()).await {
            Ok(()) => Ok(Ok(())),
            Err(err) => Ok(Err(store::Error::Other(err.to_string()))),
        }
    }

    #[instrument(level = "trace", skip(self), ret(level = "trace"))]
    async fn delete(
        &self,
        bucket: ResourceBorrow<store::Bucket>,
        key: String,
    ) -> anyhow::Result<Result<(), store::Error>> {
        let mut conn = match self.bucket(bucket).await {
            Ok(conn) => conn,
            Err(err) => return Ok(Err(err)),
        };
        match conn.del(key).await {
            Ok(()) => Ok(Ok(())),
            Err(err) => Ok(Err(store::Error::Other(err.to_string()))),
        }
    }

    #[instrument(level = "trace", skip(self), ret(level = "trace"))]
    async fn exists(
        &self,
        bucket: ResourceBorrow<store::Bucket>,
        key: String,
    ) -> anyhow::Result<Result<bool, store::Error>> {
        let mut conn = match self.bucket(bucket).await {
            Ok(conn) => conn,
            Err(err) => return Ok(Err(err)),
        };
        match conn.exists(key).await {
            Ok(ok) => Ok(Ok(ok)),
            Err(err) => Ok(Err(store::Error::Other(err.to_string()))),
        }
    }

    #[instrument(level = "trace", skip(self), ret(level = "trace"))]
    async fn list_keys(
        &self,
        bucket: ResourceBorrow<store::Bucket>,
        cursor: Option<String>,
    ) -> anyhow::Result<Result<store::KeyResponse, store::Error>> {
        Ok(Err(store::Error::Other("not supported".into())))
    }
}

impl atomics::Handler for Handler {
    async fn increment(
        &self,
        _bucket: ResourceBorrow<store::Bucket>,
        _key: String,
        _delta: i64,
    ) -> anyhow::Result<Result<i64, store::Error>> {
        Ok(Err(store::Error::Other("not supported".into())))
    }

    async fn swap(
        &self,
        _cas: ResourceOwn<atomics::Cas>,
        _value: Bytes,
    ) -> anyhow::Result<Result<(), atomics::CasError>> {
        Ok(Err(atomics::CasError::StoreError(store::Error::Other(
            "not supported".into(),
        ))))
    }
}

impl atomics::HandlerCas for Handler {
    async fn new(
        &self,
        _bucket: ResourceBorrow<store::Bucket>,
        _key: String,
    ) -> anyhow::Result<Result<ResourceOwn<atomics::Cas>, store::Error>> {
        Ok(Err(store::Error::Other("not supported".into())))
    }

    async fn current(
        &self,
        _bucket: ResourceBorrow<atomics::Cas>,
    ) -> anyhow::Result<Result<Option<Bytes>, store::Error>> {
        Ok(Err(store::Error::Other("not supported".into())))
    }
}

impl batch::Handler for Handler {
    async fn get_many(
        &self,
        _bucket: ResourceBorrow<store::Bucket>,
        _keys: Vec<String>,
    ) -> anyhow::Result<Result<Vec<Option<(String, Bytes)>>, store::Error>> {
        Ok(Err(store::Error::Other("not supported".into())))
    }

    async fn set_many(
        &self,
        _bucket: ResourceBorrow<store::Bucket>,
        _key_values: Vec<(String, Bytes)>,
    ) -> anyhow::Result<Result<(), store::Error>> {
        Ok(Err(store::Error::Other("not supported".into())))
    }

    async fn delete_many(
        &self,
        _bucket: ResourceBorrow<store::Bucket>,
        _keys: Vec<String>,
    ) -> anyhow::Result<Result<(), store::Error>> {
        Ok(Err(store::Error::Other("not supported".into())))
    }
}
