pub mod lmdb;
pub mod postgres;

use crate::analytics::AnalyticsSink;
use crate::auth::ApiToken;
use crate::pixel::PixelConfig;
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
    /// Maximum number of visits before the link expires (`410 Gone`).
    /// `None` (the default, and every pre-existing link) means unlimited.
    /// `#[serde(default)]` is load-bearing: old persisted blobs without this
    /// field must deserialize to `None`, not fail.
    #[serde(default)]
    pub max_visits: Option<u32>,
    /// Geo/device redirect rules (roadmap #12). `#[serde(default)]` so that
    /// old blobs/rows without this field deserialize to an empty `Vec`
    /// (regression: pre-existing Records must keep working unchanged).
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// A/B destination variants (roadmap #17). `#[serde(default)]` so that
    /// old blobs/rows without this field deserialize to an empty `Vec`.
    #[serde(default)]
    pub variants: Vec<Variant>,
    /// Deep-link app destinations (roadmap #20): when set, a matching mobile
    /// platform is redirected here instead of the web `url`. `#[serde(default,
    /// skip_serializing_if)]` so old blobs deserialize to `None` and the field
    /// is omitted when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_ios: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_android: Option<String>,
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

/// Finds the index of the first rule (in order) that matches the visitor's
/// `country`/`ua`, or `None` if no rule matches (including the empty-rules
/// case). Pure (no I/O); returns an index rather than a reference so callers
/// can decide independently whether to clone the match or move the link's
/// default `url` — this is what lets the redirect hot path stay
/// clone-free when a link has no rules.
pub fn matched_rule_index(
    rules: &[Rule],
    country: Option<&str>,
    ua: Option<&str>,
) -> Option<usize> {
    if rules.is_empty() {
        return None;
    }
    let country_upper = country.map(|c| c.to_ascii_uppercase());
    let device = crate::analytics::device_from_ua(ua);
    rules.iter().position(|rule| match rule.field {
        RuleField::Country => match &country_upper {
            Some(c) => rule.values.iter().any(|v| v.eq_ignore_ascii_case(c)),
            None => false,
        },
        RuleField::Device => rule.values.iter().any(|v| v.eq_ignore_ascii_case(device)),
    })
}

/// Resolves the redirect destination for a click: with no rules (the common
/// case, every pre-existing link), returns `&rec.url` with just a
/// `Vec::is_empty()` check — no extra cost. With rules, evaluates them in
/// order and returns the first match's `to`; falls back to `&rec.url` if
/// none match. Pure (no I/O): reuses the `country`/`user_agent` already read
/// for the click's `ClickEvent`. Kept for tests/callers that want a borrowed
/// destination; the redirect handler uses `matched_rule_index` directly so it
/// can move `rec.url` instead of cloning it.
pub fn resolve_destination<'a>(
    rec: &'a Record,
    country: Option<&str>,
    ua: Option<&str>,
) -> &'a str {
    match matched_rule_index(&rec.rules, country, ua) {
        Some(i) => &rec.rules[i].to,
        None => &rec.url,
    }
}

/// One A/B destination: a URL and its relative weight (>= 1) in the
/// weighted-random pick performed at redirect time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Variant {
    pub url: String,
    pub weight: u32,
}

/// Deterministic weighted pick over `variants` given a random `u64`. Pure and
/// stateless: the caller supplies the randomness (one `getrandom` draw per
/// redirect), so this stays unit-testable with boundary values. A weight of 0
/// is treated as the minimum of 1 (defensive; validation should already
/// enforce weight >= 1 at create/patch time).
pub fn pick_variant(variants: &[Variant], rand: u64) -> usize {
    if variants.is_empty() {
        return 0;
    }
    let total: u64 = variants.iter().map(|v| v.weight.max(1) as u64).sum();
    let mut r = rand % total;
    for (i, v) in variants.iter().enumerate() {
        let w = v.weight.max(1) as u64;
        if r < w {
            return i;
        }
        r -= w;
    }
    variants.len() - 1
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
    /// Atomically increments the visit counter for `id` and returns the new
    /// total. Separate from `Record` so that a hit doesn't require rewriting
    /// the whole record. Only called for links that opted into `max_visits`.
    async fn bump_visits(&self, id: u64) -> Result<u64, StoreError>;
    /// Reads the current visit count for `id` (0 if never bumped), for display.
    async fn visits(&self, id: u64) -> Result<u64, StoreError>;
    async fn next_pixel_id(&self) -> Result<u64, StoreError>;
    async fn get_pixel(&self, id: u64) -> Result<Option<PixelConfig>, StoreError>;
    async fn put_pixel(&self, config: &PixelConfig) -> Result<(), StoreError>;
    async fn delete_pixel(&self, id: u64) -> Result<bool, StoreError>;
    async fn list_pixels(&self) -> Result<Vec<PixelConfig>, StoreError>;
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

    /// Regression: a `Record` blob persisted before `max_visits` existed (no
    /// such key in the JSON) must deserialize with `max_visits: None`, not fail.
    #[test]
    fn record_without_max_visits_field_deserializes_to_none() {
        let old_blob = r#"{"url":"https://example.com","expiry":null,"created":100}"#;
        let rec: Record = serde_json::from_str(old_blob).unwrap();
        assert_eq!(rec.max_visits, None);
        assert_eq!(rec.url, "https://example.com");
        assert_eq!(rec.created, 100);
    }

    #[test]
    fn record_with_max_visits_field_round_trips() {
        let json = r#"{"url":"https://example.com","expiry":null,"created":100,"max_visits":5}"#;
        let rec: Record = serde_json::from_str(json).unwrap();
        assert_eq!(rec.max_visits, Some(5));
    }
}

#[cfg(test)]
mod rules_tests {
    use super::{resolve_destination, Record, Rule, RuleField};

    fn rec(url: &str, rules: Vec<Rule>) -> Record {
        Record {
            url: url.into(),
            expiry: None,
            created: 0,
            tags: Vec::new(),
            max_visits: None,
            rules,
            variants: Vec::new(),
            app_ios: None,
            app_android: None,
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

#[cfg(test)]
mod variants_tests {
    use super::{pick_variant, Record, Variant};

    fn v(url: &str, weight: u32) -> Variant {
        Variant {
            url: url.to_string(),
            weight,
        }
    }

    #[test]
    fn pick_variant_equal_weights_splits_at_the_half() {
        let variants = vec![v("https://a.com", 1), v("https://b.com", 1)];
        assert_eq!(pick_variant(&variants, 0), 0);
        assert_eq!(pick_variant(&variants, 1), 1);
    }

    #[test]
    fn pick_variant_skewed_weights_favor_the_heavier_bucket() {
        let variants = vec![v("https://a.com", 3), v("https://b.com", 1)];
        assert_eq!(pick_variant(&variants, 0), 0);
        assert_eq!(pick_variant(&variants, 1), 0);
        assert_eq!(pick_variant(&variants, 2), 0);
        assert_eq!(pick_variant(&variants, 3), 1);
        assert_eq!(pick_variant(&variants, 4), 0);
        assert_eq!(pick_variant(&variants, 7), 1);
    }

    #[test]
    fn pick_variant_single_variant_always_zero() {
        let variants = vec![v("https://only.com", 5)];
        assert_eq!(pick_variant(&variants, 0), 0);
        assert_eq!(pick_variant(&variants, 999), 0);
    }

    #[test]
    fn pick_variant_empty_is_safe() {
        assert_eq!(pick_variant(&[], 42), 0);
    }

    #[test]
    fn record_without_variants_field_deserializes_to_empty_vec() {
        // Old blob shape (pre-A/B), no `variants` key at all.
        let old = r#"{"url":"https://old.com","expiry":null,"created":100}"#;
        let rec: Record = serde_json::from_str(old).unwrap();
        assert_eq!(rec.url, "https://old.com");
        assert!(rec.variants.is_empty());
    }
}
