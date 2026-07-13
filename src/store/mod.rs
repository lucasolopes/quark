pub mod lmdb;
pub mod postgres;

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
    Backend(String),
}
impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Db(e) => write!(f, "db: {e}"),
            StoreError::Serde(e) => write!(f, "serde: {e}"),
            StoreError::Backend(s) => write!(f, "backend: {s}"),
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

/// Abre só o Store em LMDB (usado por testes que não precisam do AnalyticsSink).
pub async fn open_store(path: &Path) -> Result<Arc<dyn Store>, StoreError> {
    Ok(Arc::new(lmdb::LmdbStore::open(path)?))
}

/// Par de backends (Store + AnalyticsSink) que compartilham o mesmo backend físico.
pub type Backends = (Arc<dyn Store>, Arc<dyn AnalyticsSink>);

/// Seam de seleção de backend por `QUARK_DATABASE_URL`: definido → Postgres;
/// ausente → LMDB local em `data_path`. Async pra acomodar setup de conexão
/// (Postgres) sem gambiarra de bloqueio.
pub async fn open_backends(data_path: &Path) -> Result<Backends, StoreError> {
    match std::env::var("QUARK_DATABASE_URL") {
        Ok(url) => {
            let pg = Arc::new(postgres::PostgresStore::open(&url).await?);
            let store: Arc<dyn Store> = pg.clone();
            let sink: Arc<dyn AnalyticsSink> = pg;
            Ok((store, sink))
        }
        Err(_) => {
            let lmdb = Arc::new(lmdb::LmdbStore::open(data_path)?);
            let store: Arc<dyn Store> = lmdb.clone();
            let sink: Arc<dyn AnalyticsSink> = lmdb;
            Ok((store, sink))
        }
    }
}
