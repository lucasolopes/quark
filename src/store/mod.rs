pub mod lmdb;
pub mod postgres;

use crate::analytics::AnalyticsSink;
use crate::auth::ApiToken;
use crate::webhooks::WebhookSubscription;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub url: String,
    pub expiry: Option<u64>,
    pub created: u64,
    /// Free-form labels for organizing/filtering links. Absent on older
    /// persisted blobs, hence `#[serde(default)]` -> `[]`.
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Maximum number of tags kept per link (extra tags beyond this are dropped).
const MAX_TAGS: usize = 20;
/// Maximum length (in chars) kept per tag (longer tags are truncated).
const MAX_TAG_CHARS: usize = 40;

/// Normalizes a raw list of tags into the canonical stored form: each tag is
/// trimmed, lowercased, and truncated to `MAX_TAG_CHARS` chars; empty tags are
/// dropped; duplicates are removed (first occurrence wins, order preserved);
/// the result is capped at `MAX_TAGS` entries.
pub fn normalize_tags(raw: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for t in raw {
        let trimmed = t.trim().to_lowercase();
        if trimmed.is_empty() {
            continue;
        }
        let capped: String = trimmed.chars().take(MAX_TAG_CHARS).collect();
        if seen.insert(capped.clone()) {
            out.push(capped);
            if out.len() >= MAX_TAGS {
                break;
            }
        }
    }
    out
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
    /// `tag`, when present, restricts the results to links whose `tags`
    /// contain it (exact match, post-normalization).
    async fn list_links(
        &self,
        after: Option<u64>,
        limit: usize,
        tag: Option<&str>,
    ) -> Result<Vec<(u64, Record)>, StoreError>;
    /// Paginated server-side search (keyset by id). Matches `url`/`alias`,
    /// case-insensitive, literal term. Backends without search return
    /// `Err(StoreError::Unsupported)`. `tag` narrows the results as in
    /// `list_links`.
    async fn search_links(
        &self,
        q: &str,
        after: Option<u64>,
        limit: usize,
        tag: Option<&str>,
    ) -> Result<Vec<(u64, Record)>, StoreError>;
    async fn list_aliases(&self) -> Result<Vec<(String, u64)>, StoreError>;
    /// Distinct set of tags across all links, sorted.
    async fn list_tags(&self) -> Result<Vec<String>, StoreError>;
    async fn delete_link(&self, id: u64) -> Result<(), StoreError>;
    async fn delete_alias(&self, alias: &str) -> Result<(), StoreError>;
    async fn list_webhooks(&self) -> Result<Vec<WebhookSubscription>, StoreError>;
    async fn get_webhook(&self, id: u64) -> Result<Option<WebhookSubscription>, StoreError>;
    async fn put_webhook(&self, sub: &WebhookSubscription) -> Result<(), StoreError>;
    async fn delete_webhook(&self, id: u64) -> Result<bool, StoreError>;
    async fn next_webhook_id(&self) -> Result<u64, StoreError>;
    async fn list_api_tokens(&self) -> Result<Vec<ApiToken>, StoreError>;
    async fn get_api_token_by_hash(&self, hash: &str) -> Result<Option<ApiToken>, StoreError>;
    async fn put_api_token(&self, token: &ApiToken) -> Result<(), StoreError>;
    async fn delete_api_token(&self, id: u64) -> Result<bool, StoreError>;
    async fn next_api_token_id(&self) -> Result<u64, StoreError>;
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

#[cfg(test)]
mod tests {
    use super::{normalize_tags, Record};

    #[test]
    fn normalize_tags_trims_lowercases_and_drops_empties() {
        assert_eq!(
            normalize_tags(vec![" Rust ".into(), "".into(), "  ".into(), "WEB".into()]),
            vec!["rust".to_string(), "web".to_string()]
        );
    }

    #[test]
    fn normalize_tags_dedupes_preserving_first_order() {
        assert_eq!(
            normalize_tags(vec!["a".into(), "b".into(), "A".into(), "a".into()]),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn normalize_tags_caps_count_at_20() {
        let raw: Vec<String> = (0..30).map(|i| format!("t{i}")).collect();
        let out = normalize_tags(raw);
        assert_eq!(out.len(), 20);
        assert_eq!(out[0], "t0");
        assert_eq!(out[19], "t19");
    }

    #[test]
    fn normalize_tags_caps_length_at_40_chars() {
        let long = "a".repeat(50);
        let out = normalize_tags(vec![long]);
        assert_eq!(out[0].len(), 40);
    }

    #[test]
    fn record_deserializes_without_tags_field_as_empty() {
        let old_blob = r#"{"url":"https://example.com","expiry":null,"created":1}"#;
        let rec: Record = serde_json::from_str(old_blob).unwrap();
        assert_eq!(rec.url, "https://example.com");
        assert!(rec.tags.is_empty());
    }
}
