use crate::store::{Record, Store, StoreError};
use moka::sync::Cache as Moka;
use std::sync::Arc;

pub struct Cache {
    store: Arc<dyn Store>,
    hot: Moka<u64, Record>,
}

impl Cache {
    pub fn new(store: Arc<dyn Store>, capacity: u64) -> Cache {
        Cache {
            store,
            hot: Moka::new(capacity),
        }
    }

    pub async fn get(&self, id: u64) -> Result<Option<Record>, StoreError> {
        if let Some(rec) = self.hot.get(&id) {
            return Ok(Some(rec));
        }
        match self.store.get_link(id).await? {
            Some(rec) => {
                self.hot.insert(id, rec.clone());
                Ok(Some(rec))
            }
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::lmdb::LmdbStore;
    use crate::store::{Record, Store};
    use std::sync::Arc;

    #[tokio::test]
    async fn hit_e_miss() {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn Store> = Arc::new(LmdbStore::open(dir.path()).unwrap());
        store
            .put_link(
                3,
                &Record {
                    url: "u".into(),
                    expiry: None,
                    created: 0,
                },
            )
            .await
            .unwrap();
        let cache = Cache::new(store.clone(), 1000);
        assert_eq!(cache.get(3).await.unwrap().unwrap().url, "u"); // miss → popula
        assert_eq!(cache.get(3).await.unwrap().unwrap().url, "u"); // hit
        assert!(cache.get(404).await.unwrap().is_none());
    }
}
