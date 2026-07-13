pub mod lmdb;

use crate::analytics::AnalyticsSink;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub url: String,
    pub expiry: Option<u64>,
    pub created: u64,
}

#[derive(Debug)]
pub enum StoreError {
    Db(heed::Error),
    Serde(serde_json::Error),
}
impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Db(e) => write!(f, "db: {e}"),
            StoreError::Serde(e) => write!(f, "serde: {e}"),
        }
    }
}
impl std::error::Error for StoreError {}
impl From<heed::Error> for StoreError {
    fn from(e: heed::Error) -> Self {
        StoreError::Db(e)
    }
}
impl From<serde_json::Error> for StoreError {
    fn from(e: serde_json::Error) -> Self {
        StoreError::Serde(e)
    }
}

/// Interface de persistência. O caminho quente é sempre servido do cache L1;
/// os métodos async permitem backends de rede (Postgres/Valkey) nos próximos
/// tijolos sem gambiarra de bloqueio.
#[async_trait::async_trait]
pub trait Store: Send + Sync + 'static {
    async fn next_id(&self) -> Result<u64, StoreError>;
    async fn get_link(&self, id: u64) -> Result<Option<Record>, StoreError>;
    async fn put_link(&self, id: u64, rec: &Record) -> Result<(), StoreError>;
    async fn get_alias(&self, alias: &str) -> Result<Option<u64>, StoreError>;
    async fn put_alias_and_link(
        &self,
        alias: &str,
        id: u64,
        rec: &Record,
    ) -> Result<bool, StoreError>;
}

/// Seam de seleção de backend. Hoje só resolve LMDB; o Tijolo 4 adiciona o
/// match em `QUARK_STORE`. Async pra acomodar setup de conexão (Postgres) depois.
pub async fn open_store(path: &Path) -> Result<Arc<dyn Store>, StoreError> {
    Ok(Arc::new(lmdb::LmdbStore::open(path)?))
}

/// Par de backends (Store + AnalyticsSink) que compartilham o mesmo env LMDB.
pub type Backends = (Arc<dyn Store>, Arc<dyn AnalyticsSink>);

/// Abre UM LmdbStore e o expõe como Store E AnalyticsSink (mesmo env LMDB).
pub fn open_backends(path: &Path) -> Result<Backends, StoreError> {
    let backend = Arc::new(lmdb::LmdbStore::open(path)?);
    let store: Arc<dyn Store> = backend.clone();
    let sink: Arc<dyn AnalyticsSink> = backend;
    Ok((store, sink))
}
