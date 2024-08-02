mod bindings {
    use crate::Handler;

    wit_bindgen::generate!({
       with: {
           "wasi:keyvalue/store@0.2.0-draft": generate
       }
    });

    export!(Handler);
}

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use bindings::exports::wasi::keyvalue::store::{self, KeyResponse};

type Result<T> = core::result::Result<T, store::Error>;

pub struct Handler;

fn map_lock<'a, T, U>(
    lock: &'a RwLock<T>,
    f: impl FnOnce(&'a RwLock<T>) -> core::result::Result<U, PoisonError<U>>,
) -> Result<U> {
    f(lock).map_err(|err| store::Error::Other(err.to_string()))
}

#[derive(Clone, Default)]
pub struct Bucket(Arc<RwLock<HashMap<String, Vec<u8>>>>);

impl Bucket {
    fn read(&self) -> Result<RwLockReadGuard<'_, HashMap<String, Vec<u8>>>> {
        map_lock(&self.0, RwLock::read)
    }

    fn write(&self) -> Result<RwLockWriteGuard<'_, HashMap<String, Vec<u8>>>> {
        map_lock(&self.0, RwLock::write)
    }
}

impl bindings::exports::wasi::keyvalue::store::GuestBucket for Bucket {
    fn get(&self, key: String) -> Result<Option<Vec<u8>>> {
        let bucket = self.read()?;
        Ok(bucket.get(&key).cloned())
    }

    fn set(&self, key: String, value: Vec<u8>) -> Result<()> {
        let mut bucket = self.write()?;
        bucket.insert(key, value);
        Ok(())
    }

    fn delete(&self, key: String) -> Result<()> {
        let mut bucket = self.write()?;
        bucket.remove(&key);
        Ok(())
    }

    fn exists(&self, key: String) -> Result<bool> {
        let bucket = self.read()?;
        Ok(bucket.contains_key(&key))
    }

    fn list_keys(&self, cursor: Option<u64>) -> Result<KeyResponse> {
        let bucket = self.read()?;
        let bucket = bucket.keys();
        let keys = if let Some(cursor) = cursor {
            let cursor =
                usize::try_from(cursor).map_err(|err| store::Error::Other(err.to_string()))?;
            bucket.skip(cursor).cloned().collect()
        } else {
            bucket.cloned().collect()
        };
        Ok(KeyResponse { keys, cursor: None })
    }
}

impl bindings::exports::wasi::keyvalue::store::Guest for Handler {
    type Bucket = Bucket;

    fn open(identifier: String) -> Result<store::Bucket> {
        static STORE: OnceLock<Mutex<HashMap<String, Bucket>>> = OnceLock::new();
        let store = STORE.get_or_init(Mutex::default);
        let mut store = store.lock().expect("failed to lock store");
        let bucket = store.entry(identifier).or_default().clone();
        Ok(store::Bucket::new(bucket))
    }
}
