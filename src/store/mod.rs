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
    /// Geo/device redirect rules (roadmap #12). `#[serde(default)]` so that
    /// old blobs/rows without this field deserialize to an empty `Vec`
    /// (regression: pre-existing Records must keep working unchanged).
    #[serde(default)]
    pub rules: Vec<Rule>,
}

/// A single geo/device redirect rule: if the visitor's `field` value is in
/// `values`, the redirect goes to `to` instead of the link's default `url`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Rule {
    pub field: RuleField,
    pub values: Vec<String>,
    pub to: String,
}

/// The visitor attribute a `Rule` matches on. OS/browser rules are out of
/// scope for this task (need finer UA parsers not yet on main).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RuleField {
    Country,
    Device,
}

/// Resolves the redirect destination for a click: with no rules (the common
/// case, every pre-existing link), returns `&rec.url` with just a
/// `Vec::is_empty()` check — no extra cost. With rules, evaluates them in
/// order and returns the first match's `to`; falls back to `&rec.url` if
/// none match. Pure (no I/O): reuses the `country`/`user_agent` already read
/// for the click's `ClickEvent`.
pub fn resolve_destination<'a>(
    rec: &'a Record,
    country: Option<&str>,
    ua: Option<&str>,
) -> &'a str {
    if rec.rules.is_empty() {
        return &rec.url;
    }
    let country_upper = country.map(|c| c.to_ascii_uppercase());
    let device = crate::analytics::device_from_ua(ua);
    for rule in &rec.rules {
        let matched = match rule.field {
            RuleField::Country => match &country_upper {
                Some(c) => rule.values.iter().any(|v| v.eq_ignore_ascii_case(c)),
                None => false,
            },
            RuleField::Device => rule.values.iter().any(|v| v.eq_ignore_ascii_case(device)),
        };
        if matched {
            return &rule.to;
        }
    }
    &rec.url
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
mod rules_tests {
    use super::{resolve_destination, Record, Rule, RuleField};

    fn rec(url: &str, rules: Vec<Rule>) -> Record {
        Record {
            url: url.into(),
            expiry: None,
            created: 0,
            rules,
        }
    }

    #[test]
    fn no_rules_returns_default_url() {
        let r = rec("https://default.example", vec![]);
        assert_eq!(
            resolve_destination(&r, Some("BR"), None),
            "https://default.example"
        );
    }

    #[test]
    fn country_rule_matches_uppercased() {
        let r = rec(
            "https://default.example",
            vec![Rule {
                field: RuleField::Country,
                values: vec!["BR".into()],
                to: "https://br.example".into(),
            }],
        );
        assert_eq!(
            resolve_destination(&r, Some("br"), None),
            "https://br.example"
        );
        assert_eq!(
            resolve_destination(&r, Some("US"), None),
            "https://default.example"
        );
        assert_eq!(
            resolve_destination(&r, None, None),
            "https://default.example"
        );
    }

    #[test]
    fn device_rule_matches_via_device_from_ua() {
        let r = rec(
            "https://default.example",
            vec![Rule {
                field: RuleField::Device,
                values: vec!["Mobile".into()],
                to: "https://m.example".into(),
            }],
        );
        assert_eq!(
            resolve_destination(&r, None, Some("Mozilla/5.0 (iPhone; CPU iPhone OS)")),
            "https://m.example"
        );
        assert_eq!(
            resolve_destination(&r, None, Some("Mozilla/5.0 (Windows NT 10.0; Win64)")),
            "https://default.example"
        );
    }

    #[test]
    fn first_matching_rule_wins() {
        let r = rec(
            "https://default.example",
            vec![
                Rule {
                    field: RuleField::Country,
                    values: vec!["BR".into()],
                    to: "https://first.example".into(),
                },
                Rule {
                    field: RuleField::Country,
                    values: vec!["BR".into()],
                    to: "https://second.example".into(),
                },
            ],
        );
        assert_eq!(
            resolve_destination(&r, Some("BR"), None),
            "https://first.example"
        );
    }

    #[test]
    fn old_blob_without_rules_deserializes_to_empty_vec() {
        let old_json = r#"{"url":"https://old.example","expiry":null,"created":0}"#;
        let r: Record = serde_json::from_str(old_json).unwrap();
        assert!(r.rules.is_empty());
        assert_eq!(
            resolve_destination(&r, Some("BR"), None),
            "https://old.example"
        );
    }
}
