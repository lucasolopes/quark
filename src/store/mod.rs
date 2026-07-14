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
    IdSpaceExhausted,
    /// Operation not supported by this backend (e.g. server-side search on LMDB).
    Unsupported,
}
impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Db(e) => write!(f, "db: {e}"),
            StoreError::Serde(e) => write!(f, "serde: {e}"),
            StoreError::Backend(s) => write!(f, "backend: {s}"),
            StoreError::IdSpaceExhausted => write!(f, "id space exhausted"),
            StoreError::Unsupported => write!(f, "operation not supported by this backend"),
        }
    }
}
impl std::error::Error for StoreError {}
impl StoreError {
    /// Builds a `Backend` from any displayable error (sqlx,
    /// clickhouse, etc). Shortens the repeated `.map_err(|e| Backend(e.to_string()))`
    /// in the network backends: `.map_err(StoreError::backend)`.
    pub fn backend<E: std::fmt::Display>(e: E) -> StoreError {
        StoreError::Backend(e.to_string())
    }
}
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

/// Persistence interface. The hot path is always served from the L1 cache;
/// the async methods accommodate network backends (Postgres/Valkey) without a
/// blocking workaround.
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
    async fn add_blocked_domain(&self, domain: &str) -> Result<(), StoreError>;
    async fn remove_blocked_domain(&self, domain: &str) -> Result<(), StoreError>;
    async fn list_blocked_domains(&self) -> Result<Vec<String>, StoreError>;
    async fn list_links(
        &self,
        after: Option<u64>,
        limit: usize,
    ) -> Result<Vec<(u64, Record)>, StoreError>;
    /// Paginated server-side search (keyset by id). Matches `url`/`alias`,
    /// case-insensitive, literal term. Backends without search return
    /// `Err(StoreError::Unsupported)`.
    async fn search_links(
        &self,
        q: &str,
        after: Option<u64>,
        limit: usize,
    ) -> Result<Vec<(u64, Record)>, StoreError>;
    async fn list_aliases(&self) -> Result<Vec<(String, u64)>, StoreError>;
    async fn delete_link(&self, id: u64) -> Result<(), StoreError>;
    async fn delete_alias(&self, alias: &str) -> Result<(), StoreError>;
    /// Reads a well-known app-association document by name. The raw JSON is
    /// returned verbatim (no parsing here; validation lives at the HTTP layer).
    async fn get_wellknown(&self, name: &str) -> Result<Option<String>, StoreError>;
    /// Stores a well-known document, replacing any existing body for `name`.
    async fn put_wellknown(&self, name: &str, body: &str) -> Result<(), StoreError>;
    /// Deletes a well-known document; a missing document is not an error.
    async fn delete_wellknown(&self, name: &str) -> Result<(), StoreError>;
}

/// Opens only the Store on LMDB (used by tests that don't need the AnalyticsSink).
pub async fn open_store(path: &Path) -> Result<Arc<dyn Store>, StoreError> {
    Ok(Arc::new(lmdb::LmdbStore::open(path)?))
}

/// Pair of backends (Store + AnalyticsSink) sharing the same physical backend.
pub type Backends = (Arc<dyn Store>, Arc<dyn AnalyticsSink>);

/// Backend-selection seam via `QUARK_DATABASE_URL`: set -> Postgres;
/// absent -> local LMDB at `data_path`. Async to accommodate connection setup
/// (Postgres) without a blocking workaround.
///
/// The Store and the AnalyticsSink are chosen independently: the Store
/// (+ its embedded sink) follows the rule above; the Sink is overridden by
/// `QUARK_CLICKHOUSE_URL` when set (ClickHouse is analytics-only,
/// never a Store).
pub async fn open_backends(data_path: &Path) -> Result<Backends, StoreError> {
    let (store, embedded_sink): (Arc<dyn Store>, Arc<dyn AnalyticsSink>) =
        match std::env::var("QUARK_DATABASE_URL") {
            Ok(url) => {
                let pg = Arc::new(postgres::PostgresStore::open(&url).await?);
                (pg.clone(), pg)
            }
            Err(_) => {
                let lmdb = Arc::new(lmdb::LmdbStore::open(data_path)?);
                (lmdb.clone(), lmdb)
            }
        };
    let sink: Arc<dyn AnalyticsSink> = match std::env::var("QUARK_CLICKHOUSE_URL") {
        Ok(url) => Arc::new(crate::analytics::clickhouse::ClickHouseSink::open(&url).await?),
        Err(_) => embedded_sink,
    };
    Ok((store, sink))
}
