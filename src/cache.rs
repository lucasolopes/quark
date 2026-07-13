use crate::store::{Record, Store, StoreError};
use moka::sync::Cache as Moka;
use std::sync::Arc;

pub struct Cache {
    store: Arc<Store>,
    hot: Moka<u64, Record>,
}

impl Cache {
    pub fn new(store: Arc<Store>, capacity: u64) -> Cache {
        Cache {
            store,
            hot: Moka::new(capacity),
        }
    }

    pub fn get(&self, id: u64) -> Result<Option<Record>, StoreError> {
        if let Some(rec) = self.hot.get(&id) {
            return Ok(Some(rec));
        }
        match self.store.get_link(id)? {
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
    use crate::store::{Record, Store};
    use std::sync::Arc;

    #[test]
    fn hit_e_miss() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap());
        store
            .put_link(
                3,
                &Record {
                    url: "u".into(),
                    expiry: None,
                    created: 0,
                },
            )
            .unwrap();
        let cache = Cache::new(store.clone(), 1000);
        assert_eq!(cache.get(3).unwrap().unwrap().url, "u"); // miss → popula
        assert_eq!(cache.get(3).unwrap().unwrap().url, "u"); // hit
        assert!(cache.get(404).unwrap().is_none());
    }
}
