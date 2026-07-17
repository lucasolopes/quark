pub mod lmdb;
pub mod postgres;

use crate::analytics::AnalyticsSink;
use crate::auth::ApiToken;
use crate::domain::{Domain, DomainStatus};
use crate::invite::Invite;
use crate::oidc::TenantOidcConfig;
use crate::pixel::PixelConfig;
use crate::tenant::{Membership, Tenant, TenantId, User};
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
    /// Optional single folder this link belongs to (roadmap: folders). A link
    /// lives in at most one folder, an exclusive counterpart to the free-form
    /// `tags`. `#[serde(default, skip_serializing_if)]` so old blobs/rows
    /// without this field deserialize to `None` and the field is omitted when
    /// absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// Optional URL to redirect to when the link has expired (by time or by
    /// visit count) instead of returning `410 Gone`. `#[serde(default,
    /// skip_serializing_if)]` so old blobs/rows without this field deserialize
    /// to `None` and the field is omitted when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_url: Option<String>,
    /// Optional argon2 PHC hash of a per-link password. When set, a visitor must
    /// pass an interstitial before the redirect. The plaintext is never stored.
    /// `#[serde(default, skip_serializing_if)]` so old blobs/rows without this
    /// field deserialize to `None` and the field is omitted when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_hash: Option<String>,
    /// The tenant that owns this link. `#[serde(default)]` so old
    /// blobs/rows without the column deserialize to `DEFAULT_TENANT` (every
    /// pre-multi-tenancy record). Carried on the `Record` itself (not just
    /// looked up separately) so the redirect hot path can compare it against
    /// the resolved `Host` route's tenant after a cache hit — the L1/L2 tiers
    /// are keyed by `id` alone (ids are globally unique), so this is the only
    /// place a cross-tenant cache hit can still be caught before it's served.
    #[serde(default)]
    pub tenant_id: TenantId,
}

/// Maximum number of tags kept per link (extra tags beyond this are dropped).
const MAX_TAGS: usize = 20;
/// Maximum length (in chars) kept per tag (longer tags are truncated).
const MAX_TAG_CHARS: usize = 40;
/// Maximum length (in chars) kept for a folder name (longer names are truncated).
const MAX_FOLDER_CHARS: usize = 48;

/// Normalizes a raw folder name into the canonical stored form: trimmed and
/// truncated to `MAX_FOLDER_CHARS` chars, with the display case preserved (so
/// names like "Marketing" round-trip); an empty or whitespace-only name becomes
/// `None`. Unlike tags, the case is kept for display; the folder filter compares
/// case-insensitively.
pub fn normalize_folder(raw: Option<String>) -> Option<String> {
    let trimmed = raw?.trim().to_string();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(MAX_FOLDER_CHARS).collect())
}

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

/// A durable webhook delivery to enqueue into the Postgres outbox: one row per
/// (event, subscription). `delivery_key` is the stable idempotency id
/// (`"<event_id>.<subscription_id>"`) that a duplicate enqueue collides on
/// (`ON CONFLICT (delivery_key) DO NOTHING`) and that the relay sends as the
/// `webhook-id` header (stable across attempts and nodes). `next_attempt_at`
/// is when the relay may first try it (usually `now` at insert time).
#[derive(Debug, Clone)]
pub struct OutboxRow {
    pub delivery_key: String,
    pub subscription_id: u64,
    pub event_type: String,
    pub payload: String,
    pub created: u64,
    pub next_attempt_at: u64,
    /// The tenant that owns the subscription this row delivers to (the
    /// link/event's tenant). Stamped by `lifecycle_deliveries` and carried
    /// through so `claim_due_deliveries` can hand it back to the relay, which
    /// resolves the subscription within the correct tenant.
    pub tenant_id: TenantId,
}

/// A claimed (leased) outbox delivery the relay is about to attempt. `id` is
/// the `BIGSERIAL` primary key used by `mark_delivered`/`mark_retry`/`mark_dead`;
/// `attempts` is the count of failed attempts so far (0 on the first try).
#[derive(Debug, Clone)]
pub struct OutboxDelivery {
    pub id: i64,
    pub delivery_key: String,
    pub subscription_id: u64,
    pub event_type: String,
    pub payload: String,
    pub attempts: u32,
    /// The tenant that owns the subscription (carried from the `OutboxRow`
    /// that was enqueued). The relay uses this to resolve the subscription
    /// within the right tenant instead of assuming `DEFAULT_TENANT`.
    pub tenant_id: TenantId,
}

#[derive(Debug)]
pub enum StoreError {
    Db(heed::Error),
    Serde(serde_json::Error),
    Backend(String),
    IdSpaceExhausted,
    /// Operation not supported by this backend (e.g. server-side search on LMDB).
    Unsupported,
    /// A unique-constraint violation (e.g. duplicate tenant `slug`). Kept
    /// distinct from `Backend` so callers can map it to `409 Conflict`
    /// instead of `503`.
    UniqueViolation,
}
impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Db(e) => write!(f, "db: {e}"),
            StoreError::Serde(e) => write!(f, "serde: {e}"),
            StoreError::Backend(s) => write!(f, "backend: {s}"),
            StoreError::IdSpaceExhausted => write!(f, "id space exhausted"),
            StoreError::Unsupported => write!(f, "operation not supported by this backend"),
            StoreError::UniqueViolation => write!(f, "unique constraint violated"),
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

/// Health of a link's destination, recorded by the background checker
/// (broken-link monitoring). Kept off `Record` so a probe every sweep does not
/// rewrite the whole link record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkHealth {
    /// Unix seconds of the last probe.
    pub checked_at: u64,
    /// HTTP status observed, or `None` on a connection error / timeout.
    pub status: Option<u16>,
    pub healthy: bool,
}

/// Persistence interface. The hot path is always served from the L1 cache;
/// the async methods accommodate network backends (Postgres/Valkey) without a
/// blocking workaround.
#[async_trait::async_trait]
pub trait Store: Send + Sync + 'static {
    async fn next_id(&self, tenant: TenantId) -> Result<u64, StoreError>;
    async fn get_link(&self, tenant: TenantId, id: u64) -> Result<Option<Record>, StoreError>;
    async fn put_link(&self, tenant: TenantId, id: u64, rec: &Record) -> Result<(), StoreError>;
    /// Looks up an alias within its domain's namespace. Scoped by `domain_id`,
    /// not by tenant: a domain already picks out at most one tenant (or the
    /// shared namespace, `SHARED_DOMAIN_ID`, which by design crosses every
    /// tenant), so there is no separate tenant filter here. Mirrors
    /// `get_domain_by_host`'s bare-lookup shape for the same reason: the
    /// redirect path resolves the domain before it resolves the tenant.
    async fn get_alias(&self, domain_id: u64, alias: &str) -> Result<Option<u64>, StoreError>;
    async fn put_alias_and_link(
        &self,
        tenant: TenantId,
        domain_id: u64,
        alias: &str,
        id: u64,
        rec: &Record,
    ) -> Result<bool, StoreError>;
    /// Transactional variant of `put_link`: upserts the link AND enqueues
    /// `deliveries` (webhook outbox rows) in ONE transaction, so a crash can
    /// never persist the mutation without its lifecycle deliveries (or vice
    /// versa). The enqueue uses `ON CONFLICT (delivery_key) DO NOTHING`, so a
    /// duplicate delivery is a no-op while the link still upserts. Used by the
    /// create-numeric and patch paths. On LMDB `deliveries` is always empty
    /// (lifecycle events ride the in-memory channel) and this delegates to
    /// `put_link`, ignoring it.
    async fn put_link_tx(
        &self,
        tenant: TenantId,
        id: u64,
        rec: &Record,
        deliveries: &[OutboxRow],
    ) -> Result<(), StoreError>;
    /// Transactional variant of `put_alias_and_link`: claims the alias, puts
    /// the link, and enqueues `deliveries` in ONE transaction. Returns
    /// `Ok(false)` WITHOUT writing the link or the deliveries (the transaction
    /// rolls back) when the alias is already in use, so the enqueue is
    /// naturally conditional on the mutation succeeding. On LMDB `deliveries`
    /// is empty and this delegates to `put_alias_and_link`.
    async fn put_alias_and_link_tx(
        &self,
        tenant: TenantId,
        domain_id: u64,
        alias: &str,
        id: u64,
        rec: &Record,
        deliveries: &[OutboxRow],
    ) -> Result<bool, StoreError>;
    /// Transactional variant of `delete_link`: deletes the link AND enqueues
    /// `deliveries` in ONE transaction. On LMDB `deliveries` is empty and this
    /// delegates to `delete_link`.
    async fn delete_link_tx(
        &self,
        tenant: TenantId,
        id: u64,
        deliveries: &[OutboxRow],
    ) -> Result<(), StoreError>;
    /// `tag`, when present, restricts the results to links whose `tags`
    /// contain it (exact match, post-normalization). `folder`, when present,
    /// restricts the results to links whose `folder` matches it
    /// case-insensitively.
    async fn list_links(
        &self,
        tenant: TenantId,
        after: Option<u64>,
        limit: usize,
        tag: Option<&str>,
        folder: Option<&str>,
    ) -> Result<Vec<(u64, Record)>, StoreError>;
    /// Paginated server-side search (keyset by id). Matches `url`/`alias`,
    /// case-insensitive, literal term. Backends without search return
    /// `Err(StoreError::Unsupported)`. `tag` and `folder` narrow the results as
    /// in `list_links`.
    async fn search_links(
        &self,
        tenant: TenantId,
        q: &str,
        after: Option<u64>,
        limit: usize,
        tag: Option<&str>,
        folder: Option<&str>,
    ) -> Result<Vec<(u64, Record)>, StoreError>;
    async fn list_aliases(&self, tenant: TenantId) -> Result<Vec<(String, u64)>, StoreError>;
    /// Distinct tags across all links with their link counts, sorted by name.
    /// A link's `tags` is a `Vec<String>`; each distinct tag on a link counts
    /// that link once.
    async fn list_tags(&self, tenant: TenantId) -> Result<Vec<(String, u64)>, StoreError>;
    /// Distinct folder names across all links with their link counts, sorted by
    /// name. Links with no folder are ignored.
    async fn list_folders(&self, tenant: TenantId) -> Result<Vec<(String, u64)>, StoreError>;
    async fn delete_link(&self, tenant: TenantId, id: u64) -> Result<(), StoreError>;
    async fn delete_alias(&self, tenant: TenantId, alias: &str) -> Result<(), StoreError>;
    async fn list_webhooks(&self, tenant: TenantId)
        -> Result<Vec<WebhookSubscription>, StoreError>;
    async fn get_webhook(
        &self,
        tenant: TenantId,
        id: u64,
    ) -> Result<Option<WebhookSubscription>, StoreError>;
    async fn put_webhook(
        &self,
        tenant: TenantId,
        sub: &WebhookSubscription,
    ) -> Result<(), StoreError>;
    async fn delete_webhook(&self, tenant: TenantId, id: u64) -> Result<bool, StoreError>;
    async fn next_webhook_id(&self, tenant: TenantId) -> Result<u64, StoreError>;
    async fn list_api_tokens(&self, tenant: TenantId) -> Result<Vec<ApiToken>, StoreError>;
    /// Hash-lookup: tenant-less (the token hash is globally unique); the owning
    /// tenant travels on the row/value.
    async fn get_api_token_by_hash(&self, hash: &str) -> Result<Option<ApiToken>, StoreError>;
    async fn put_api_token(&self, tenant: TenantId, token: &ApiToken) -> Result<(), StoreError>;
    async fn delete_api_token(&self, tenant: TenantId, id: u64) -> Result<bool, StoreError>;
    async fn next_api_token_id(&self, tenant: TenantId) -> Result<u64, StoreError>;
    /// Persists an OIDC login session (keyed by its token hash). `tenant` is the
    /// owning tenant, stored on the row/value; the hash-lookup remains
    /// tenant-less.
    async fn put_session(
        &self,
        tenant: TenantId,
        session: &crate::auth::Session,
    ) -> Result<(), StoreError>;
    /// Looks up a session by its token hash. Returns `None` when absent OR
    /// expired (`expires <= now`), so an expired cookie never authenticates.
    async fn get_session_by_hash(
        &self,
        token_hash: &str,
        now: u64,
    ) -> Result<Option<crate::auth::Session>, StoreError>;
    /// Deletes a session (logout / revoke); a missing session is not an error.
    async fn delete_session(&self, token_hash: &str) -> Result<(), StoreError>;
    /// Removes all sessions that expired at or before `now`.
    async fn gc_sessions(&self, now: u64) -> Result<(), StoreError>;
    /// Atomically increments the visit counter for `id` and returns the new
    /// total. Separate from `Record` so that a hit doesn't require rewriting
    /// the whole record. Only called for links that opted into `max_visits`.
    async fn bump_visits(&self, tenant: TenantId, id: u64) -> Result<u64, StoreError>;
    /// Reads the current visit count for `id` (0 if never bumped), for display.
    async fn visits(&self, tenant: TenantId, id: u64) -> Result<u64, StoreError>;
    /// Records the latest health probe result for a link (broken-link
    /// monitoring). Upserts by id; a link is probed at most once per sweep.
    async fn put_link_health(
        &self,
        tenant: TenantId,
        id: u64,
        health: &LinkHealth,
    ) -> Result<(), StoreError>;
    /// All recorded link-health entries. Used by the checker to detect
    /// healthy<->broken transitions across the whole link set.
    async fn list_link_health(
        &self,
        tenant: TenantId,
    ) -> Result<Vec<(u64, LinkHealth)>, StoreError>;
    /// Health entries for a specific set of link ids (missing ids are simply
    /// absent from the result). Used by the admin list so a page load reads only
    /// the current page's health, not the whole table.
    async fn link_health_for(
        &self,
        tenant: TenantId,
        ids: &[u64],
    ) -> Result<Vec<(u64, LinkHealth)>, StoreError>;
    /// Ids of all links whose last probe was broken, ascending. Drives the
    /// panel's "broken only" filter without scanning the whole link table.
    async fn list_broken_link_ids(&self, tenant: TenantId) -> Result<Vec<u64>, StoreError>;
    /// Tries to acquire (or renew) the single broken-link-checker lease for
    /// `ttl_secs`, identified by `holder`. Returns `true` if this caller now
    /// holds it. Lets any replica run the checker while ensuring only one sweeps
    /// at a time; the single-node LMDB backend always returns `true`.
    async fn try_acquire_health_lease(
        &self,
        holder: &str,
        ttl_secs: u64,
    ) -> Result<bool, StoreError>;
    /// Persists the single Sheets connection (OSS is single-tenant), replacing
    /// any existing one. The `refresh_token` inside is stored server-side and is
    /// never surfaced in an API response.
    async fn put_sheets_connection(
        &self,
        tenant: TenantId,
        c: &crate::sheets::SheetsConnection,
    ) -> Result<(), StoreError>;
    /// Reads the single Sheets connection, or `None` when the connector has
    /// never been connected (or was disconnected).
    async fn get_sheets_connection(
        &self,
        tenant: TenantId,
    ) -> Result<Option<crate::sheets::SheetsConnection>, StoreError>;
    /// Removes the single Sheets connection (disconnect); a missing connection
    /// is not an error.
    async fn delete_sheets_connection(&self, tenant: TenantId) -> Result<(), StoreError>;
    /// Tries to acquire (or renew) the single scheduled-sync lease for
    /// `ttl_secs`, identified by `holder`, mirroring `try_acquire_health_lease`:
    /// only one node runs the scheduled sync at a time; the single-node LMDB
    /// backend always returns `true`.
    async fn try_acquire_sheets_lease(
        &self,
        holder: &str,
        ttl_secs: u64,
    ) -> Result<bool, StoreError>;
    async fn next_pixel_id(&self, tenant: TenantId) -> Result<u64, StoreError>;
    async fn get_pixel(&self, tenant: TenantId, id: u64)
        -> Result<Option<PixelConfig>, StoreError>;
    async fn put_pixel(&self, tenant: TenantId, config: &PixelConfig) -> Result<(), StoreError>;
    async fn delete_pixel(&self, tenant: TenantId, id: u64) -> Result<bool, StoreError>;
    async fn list_pixels(&self, tenant: TenantId) -> Result<Vec<PixelConfig>, StoreError>;
    /// Reads a well-known app-association document by name. The raw JSON is
    /// returned verbatim (no parsing here; validation lives at the HTTP layer).
    async fn get_wellknown(
        &self,
        tenant: TenantId,
        name: &str,
    ) -> Result<Option<String>, StoreError>;
    /// Stores a well-known document, replacing any existing body for `name`.
    async fn put_wellknown(
        &self,
        tenant: TenantId,
        name: &str,
        body: &str,
    ) -> Result<(), StoreError>;
    /// Deletes a well-known document; a missing document is not an error.
    async fn delete_wellknown(&self, tenant: TenantId, name: &str) -> Result<(), StoreError>;

    // --- Identity / tenancy (tenant-less; they manage tenancy itself) ---
    /// Upserts a tenant row.
    async fn put_tenant(&self, t: &Tenant) -> Result<(), StoreError>;
    /// Reads a tenant by id.
    async fn get_tenant(&self, id: TenantId) -> Result<Option<Tenant>, StoreError>;
    /// Lists every tenant. Cloud-only caller: the boot-time subdomain backfill
    /// (multi-tenancy P3), which needs to ensure every existing tenant has its
    /// `<slug>.<suffix>` `domains` row. Small table, no pagination.
    async fn list_tenants(&self) -> Result<Vec<Tenant>, StoreError>;
    /// Allocates the next global user id.
    async fn next_user_id(&self) -> Result<u64, StoreError>;
    /// Allocates the next global tenant id. Starts at 1 — 0 is the seeded default tenant.
    async fn next_tenant_id(&self) -> Result<u64, StoreError>;
    /// Upserts a global user identity (keyed by immutable OIDC subject).
    async fn put_user(&self, u: &User) -> Result<(), StoreError>;
    /// Looks up a user by OIDC subject.
    async fn get_user_by_subject(&self, subject: &str) -> Result<Option<User>, StoreError>;
    /// Looks up a user by its global id. Used by the invite accept flow to
    /// check the accepting session's email against the invite's target email.
    async fn get_user_by_id(&self, id: u64) -> Result<Option<User>, StoreError>;
    /// Upserts a membership (user <-> tenant, with role).
    async fn put_membership(&self, m: &Membership) -> Result<(), StoreError>;
    /// Reads a single membership for `(user_id, tenant)`.
    async fn get_membership(
        &self,
        user_id: u64,
        tenant: TenantId,
    ) -> Result<Option<Membership>, StoreError>;
    /// All memberships for a user, across tenants.
    async fn list_memberships_for_user(&self, user_id: u64) -> Result<Vec<Membership>, StoreError>;

    // --- Custom domains (multi-tenancy P3), cloud-only ---
    /// Allocates the next global domain id. `0` is reserved (`SHARED_DOMAIN_ID`).
    async fn next_domain_id(&self) -> Result<u64, StoreError>;
    /// Looks up a domain by host, across all tenants. Runs on the bare pool
    /// with no tenant scoping: the redirect handler only has a `Host` header
    /// before it knows which tenant owns it, so this is the one deliberately
    /// public, cross-tenant domain lookup.
    async fn get_domain_by_host(&self, host: &str) -> Result<Option<Domain>, StoreError>;
    /// Reads a domain by id, scoped to `tenant`.
    async fn get_domain(&self, tenant: TenantId, id: u64) -> Result<Option<Domain>, StoreError>;
    /// Lists all domains owned by `tenant`.
    async fn list_domains(&self, tenant: TenantId) -> Result<Vec<Domain>, StoreError>;
    /// Upserts a domain row.
    async fn put_domain(&self, domain: &Domain) -> Result<(), StoreError>;
    /// Updates a domain's verification status, scoped to `tenant`.
    async fn set_domain_status(
        &self,
        tenant: TenantId,
        id: u64,
        status: DomainStatus,
        verified_at: Option<u64>,
    ) -> Result<(), StoreError>;
    /// Deletes a domain, scoped to `tenant`.
    async fn delete_domain(&self, tenant: TenantId, id: u64) -> Result<(), StoreError>;

    // --- Team invites (multi-tenancy P2c), cloud-only ---
    /// Allocates the next global invite id.
    async fn next_invite_id(&self) -> Result<u64, StoreError>;
    /// Inserts an invite. Runs on the bare pool: the accept flow is
    /// tenant-agnostic until the invite is looked up.
    async fn create_invite(&self, inv: &Invite) -> Result<(), StoreError>;
    /// Looks up a pending, unexpired invite by its token hash, across all
    /// tenants. Runs on the bare pool with no tenant scoping (mirrors
    /// `get_domain_by_host`/`get_api_token_by_hash`): the accept flow only has
    /// the raw token before it knows which tenant the invite belongs to.
    /// Returns `None` for an invite that was already accepted
    /// (`accepted_at IS NOT NULL`) or has expired (`expires < now`).
    async fn get_invite_by_hash(
        &self,
        token_hash: &str,
        now: u64,
    ) -> Result<Option<Invite>, StoreError>;
    /// Marks an invite accepted by `accepted_by` at `now`. Runs on the bare
    /// pool, same reasoning as `create_invite`/`get_invite_by_hash`.
    ///
    /// This is the single-use claim: the update only takes effect when the
    /// row is still pending (`accepted_at IS NULL`), so two concurrent
    /// accepts of the same token race on this one row and only one wins.
    /// Returns `true` when this call claimed the row (`rows_affected() ==
    /// 1`), `false` when it lost the race or the invite was already
    /// consumed. Callers must not grant membership before this returns
    /// `true`.
    async fn mark_invite_accepted(
        &self,
        id: u64,
        accepted_by: u64,
        now: u64,
    ) -> Result<bool, StoreError>;
    /// Lists pending invites owned by `tenant`.
    async fn list_invites(&self, tenant: TenantId) -> Result<Vec<Invite>, StoreError>;
    /// Deletes an invite, scoped to `tenant`.
    async fn delete_invite(&self, tenant: TenantId, id: u64) -> Result<(), StoreError>;

    // --- Per-tenant OIDC config (multi-tenancy P2d), cloud-only ---
    /// Allocates the next global oidc-config id.
    async fn next_oidc_config_id(&self) -> Result<u64, StoreError>;
    /// Upserts the tenant's OIDC config (one per tenant: UNIQUE `tenant_id`).
    /// Tenant-scoped write; the tenant to write is `cfg.tenant_id`.
    async fn put_oidc_config(&self, cfg: &TenantOidcConfig) -> Result<(), StoreError>;
    /// Reads a tenant's OIDC config, tenant-scoped (the admin CRUD path).
    async fn get_oidc_config(
        &self,
        tenant: TenantId,
    ) -> Result<Option<TenantOidcConfig>, StoreError>;
    /// Reads a tenant's OIDC config on the bare pool, with no `app.tenant_id`
    /// set: the login/callback path resolves the tenant from the URL slug
    /// before there is any session/RLS context to scope through.
    async fn get_oidc_config_bare(
        &self,
        tenant: TenantId,
    ) -> Result<Option<TenantOidcConfig>, StoreError>;
    /// Deletes a tenant's OIDC config, tenant-scoped; a missing config is not
    /// an error.
    async fn delete_oidc_config(&self, tenant: TenantId) -> Result<(), StoreError>;
    /// Looks up a tenant by its (UNIQUE) slug. Runs on the bare pool: this is
    /// how `/admin/login?org=<slug>` resolves which tenant's OIDC config to
    /// use, before any tenant context exists.
    async fn get_tenant_by_slug(&self, slug: &str) -> Result<Option<Tenant>, StoreError>;

    /// Durable webhook outbox (scale-audit #3), Postgres-only. Inserts one
    /// delivery row per (event, subscription) with `ON CONFLICT (delivery_key)
    /// DO NOTHING`, so a duplicate enqueue of the same (event, sub) is a no-op.
    /// The LMDB backend implements this as a no-op: `main.rs` never routes
    /// lifecycle events to the outbox on LMDB (the in-memory channel handles
    /// everything there), so it is never invoked.
    async fn enqueue_deliveries(&self, rows: &[OutboxRow]) -> Result<(), StoreError>;
    /// Atomically claims (leases) up to `limit` due deliveries for the relay:
    /// `dead = false AND delivered_at IS NULL AND next_attempt_at <= now`,
    /// via `FOR UPDATE SKIP LOCKED` so two relays never claim the same row.
    /// The claim pushes `next_attempt_at` out by a visibility lease, so a
    /// relay that crashes mid-delivery has the row re-claimed after the lease
    /// expires (at-least-once). LMDB returns an empty vec (never invoked).
    async fn claim_due_deliveries(
        &self,
        now: u64,
        limit: i64,
    ) -> Result<Vec<OutboxDelivery>, StoreError>;
    /// Marks a delivery delivered (sets `delivered_at`). LMDB: no-op.
    async fn mark_delivered(&self, id: i64) -> Result<(), StoreError>;
    /// Reschedules a failed delivery: persists the incremented `attempts` and
    /// the next `next_attempt_at` (exponential backoff, survives restart).
    /// LMDB: no-op.
    async fn mark_retry(
        &self,
        id: i64,
        next_attempt_at: u64,
        attempts: u32,
    ) -> Result<(), StoreError>;
    /// Dead-letters a delivery (`dead = true`): it stops being claimed. Used
    /// after `MAX_DELIVERY_ATTEMPTS`, or when the subscription no longer exists
    /// or its destination is SSRF-blocked. `attempts` records the final count
    /// on the DLQ row for observability. LMDB: no-op.
    async fn mark_dead(&self, id: i64, attempts: u32) -> Result<(), StoreError>;
}

/// A tenant-scoped view over a `Store`. Its methods mirror the tenant-owned
/// `Store` methods but capture the tenant, so a call site cannot forget it.
pub struct ScopedStore {
    inner: Arc<dyn Store>,
    tenant: TenantId,
}

impl dyn Store {
    /// Returns a handle bound to `tenant`. All tenant-owned reads/writes go
    /// through it.
    pub fn for_tenant(self: Arc<Self>, tenant: TenantId) -> ScopedStore {
        ScopedStore {
            inner: self,
            tenant,
        }
    }
}

impl ScopedStore {
    /// The tenant this handle is bound to.
    pub fn tenant(&self) -> TenantId {
        self.tenant
    }
    /// The underlying store, for the tenant-less (global/infra/hash-lookup)
    /// methods a handler may also need.
    pub fn inner(&self) -> &Arc<dyn Store> {
        &self.inner
    }

    pub async fn next_id(&self) -> Result<u64, StoreError> {
        self.inner.next_id(self.tenant).await
    }
    pub async fn get_link(&self, id: u64) -> Result<Option<Record>, StoreError> {
        self.inner.get_link(self.tenant, id).await
    }
    pub async fn put_link(&self, id: u64, rec: &Record) -> Result<(), StoreError> {
        self.inner.put_link(self.tenant, id, rec).await
    }
    /// Scoped by `domain_id`, not by this handle's tenant: alias namespaces
    /// are per-domain (see `Store::get_alias`).
    pub async fn get_alias(&self, domain_id: u64, alias: &str) -> Result<Option<u64>, StoreError> {
        self.inner.get_alias(domain_id, alias).await
    }
    pub async fn put_alias_and_link(
        &self,
        domain_id: u64,
        alias: &str,
        id: u64,
        rec: &Record,
    ) -> Result<bool, StoreError> {
        self.inner
            .put_alias_and_link(self.tenant, domain_id, alias, id, rec)
            .await
    }
    pub async fn put_link_tx(
        &self,
        id: u64,
        rec: &Record,
        deliveries: &[OutboxRow],
    ) -> Result<(), StoreError> {
        self.inner
            .put_link_tx(self.tenant, id, rec, deliveries)
            .await
    }
    pub async fn put_alias_and_link_tx(
        &self,
        domain_id: u64,
        alias: &str,
        id: u64,
        rec: &Record,
        deliveries: &[OutboxRow],
    ) -> Result<bool, StoreError> {
        self.inner
            .put_alias_and_link_tx(self.tenant, domain_id, alias, id, rec, deliveries)
            .await
    }
    pub async fn delete_link_tx(
        &self,
        id: u64,
        deliveries: &[OutboxRow],
    ) -> Result<(), StoreError> {
        self.inner.delete_link_tx(self.tenant, id, deliveries).await
    }
    pub async fn list_links(
        &self,
        after: Option<u64>,
        limit: usize,
        tag: Option<&str>,
        folder: Option<&str>,
    ) -> Result<Vec<(u64, Record)>, StoreError> {
        self.inner
            .list_links(self.tenant, after, limit, tag, folder)
            .await
    }
    pub async fn search_links(
        &self,
        q: &str,
        after: Option<u64>,
        limit: usize,
        tag: Option<&str>,
        folder: Option<&str>,
    ) -> Result<Vec<(u64, Record)>, StoreError> {
        self.inner
            .search_links(self.tenant, q, after, limit, tag, folder)
            .await
    }
    pub async fn list_aliases(&self) -> Result<Vec<(String, u64)>, StoreError> {
        self.inner.list_aliases(self.tenant).await
    }
    pub async fn list_tags(&self) -> Result<Vec<(String, u64)>, StoreError> {
        self.inner.list_tags(self.tenant).await
    }
    pub async fn list_folders(&self) -> Result<Vec<(String, u64)>, StoreError> {
        self.inner.list_folders(self.tenant).await
    }
    pub async fn delete_link(&self, id: u64) -> Result<(), StoreError> {
        self.inner.delete_link(self.tenant, id).await
    }
    pub async fn delete_alias(&self, alias: &str) -> Result<(), StoreError> {
        self.inner.delete_alias(self.tenant, alias).await
    }
    pub async fn list_webhooks(&self) -> Result<Vec<WebhookSubscription>, StoreError> {
        self.inner.list_webhooks(self.tenant).await
    }
    pub async fn get_webhook(&self, id: u64) -> Result<Option<WebhookSubscription>, StoreError> {
        self.inner.get_webhook(self.tenant, id).await
    }
    pub async fn put_webhook(&self, sub: &WebhookSubscription) -> Result<(), StoreError> {
        self.inner.put_webhook(self.tenant, sub).await
    }
    pub async fn delete_webhook(&self, id: u64) -> Result<bool, StoreError> {
        self.inner.delete_webhook(self.tenant, id).await
    }
    pub async fn next_webhook_id(&self) -> Result<u64, StoreError> {
        self.inner.next_webhook_id(self.tenant).await
    }
    pub async fn list_api_tokens(&self) -> Result<Vec<ApiToken>, StoreError> {
        self.inner.list_api_tokens(self.tenant).await
    }
    pub async fn put_api_token(&self, token: &ApiToken) -> Result<(), StoreError> {
        self.inner.put_api_token(self.tenant, token).await
    }
    pub async fn delete_api_token(&self, id: u64) -> Result<bool, StoreError> {
        self.inner.delete_api_token(self.tenant, id).await
    }
    pub async fn next_api_token_id(&self) -> Result<u64, StoreError> {
        self.inner.next_api_token_id(self.tenant).await
    }
    pub async fn put_session(&self, session: &crate::auth::Session) -> Result<(), StoreError> {
        self.inner.put_session(self.tenant, session).await
    }
    pub async fn bump_visits(&self, id: u64) -> Result<u64, StoreError> {
        self.inner.bump_visits(self.tenant, id).await
    }
    pub async fn visits(&self, id: u64) -> Result<u64, StoreError> {
        self.inner.visits(self.tenant, id).await
    }
    pub async fn put_link_health(&self, id: u64, health: &LinkHealth) -> Result<(), StoreError> {
        self.inner.put_link_health(self.tenant, id, health).await
    }
    pub async fn list_link_health(&self) -> Result<Vec<(u64, LinkHealth)>, StoreError> {
        self.inner.list_link_health(self.tenant).await
    }
    pub async fn link_health_for(&self, ids: &[u64]) -> Result<Vec<(u64, LinkHealth)>, StoreError> {
        self.inner.link_health_for(self.tenant, ids).await
    }
    pub async fn list_broken_link_ids(&self) -> Result<Vec<u64>, StoreError> {
        self.inner.list_broken_link_ids(self.tenant).await
    }
    pub async fn put_sheets_connection(
        &self,
        c: &crate::sheets::SheetsConnection,
    ) -> Result<(), StoreError> {
        self.inner.put_sheets_connection(self.tenant, c).await
    }
    pub async fn get_sheets_connection(
        &self,
    ) -> Result<Option<crate::sheets::SheetsConnection>, StoreError> {
        self.inner.get_sheets_connection(self.tenant).await
    }
    pub async fn delete_sheets_connection(&self) -> Result<(), StoreError> {
        self.inner.delete_sheets_connection(self.tenant).await
    }
    /// `cfg.tenant_id` must be this handle's tenant; the underlying store
    /// method takes it from the config, not from a separate parameter.
    pub async fn put_oidc_config(&self, cfg: &TenantOidcConfig) -> Result<(), StoreError> {
        self.inner.put_oidc_config(cfg).await
    }
    pub async fn get_oidc_config(&self) -> Result<Option<TenantOidcConfig>, StoreError> {
        self.inner.get_oidc_config(self.tenant).await
    }
    pub async fn delete_oidc_config(&self) -> Result<(), StoreError> {
        self.inner.delete_oidc_config(self.tenant).await
    }
    pub async fn next_pixel_id(&self) -> Result<u64, StoreError> {
        self.inner.next_pixel_id(self.tenant).await
    }
    pub async fn get_pixel(&self, id: u64) -> Result<Option<PixelConfig>, StoreError> {
        self.inner.get_pixel(self.tenant, id).await
    }
    pub async fn put_pixel(&self, config: &PixelConfig) -> Result<(), StoreError> {
        self.inner.put_pixel(self.tenant, config).await
    }
    pub async fn delete_pixel(&self, id: u64) -> Result<bool, StoreError> {
        self.inner.delete_pixel(self.tenant, id).await
    }
    pub async fn list_pixels(&self) -> Result<Vec<PixelConfig>, StoreError> {
        self.inner.list_pixels(self.tenant).await
    }
    pub async fn get_wellknown(&self, name: &str) -> Result<Option<String>, StoreError> {
        self.inner.get_wellknown(self.tenant, name).await
    }
    pub async fn put_wellknown(&self, name: &str, body: &str) -> Result<(), StoreError> {
        self.inner.put_wellknown(self.tenant, name, body).await
    }
    pub async fn delete_wellknown(&self, name: &str) -> Result<(), StoreError> {
        self.inner.delete_wellknown(self.tenant, name).await
    }
    pub async fn list_invites(&self) -> Result<Vec<Invite>, StoreError> {
        self.inner.list_invites(self.tenant).await
    }
    pub async fn delete_invite(&self, id: u64) -> Result<(), StoreError> {
        self.inner.delete_invite(self.tenant, id).await
    }
}

/// Opens only the Store on LMDB (used by tests that don't need the AnalyticsSink).
pub async fn open_store(path: &Path) -> Result<Arc<dyn Store>, StoreError> {
    Ok(Arc::new(lmdb::LmdbStore::open(path)?))
}

/// Opens a Postgres-backed store (runs the idempotent schema migration).
/// Returns the concrete type so tests can reach `reset_for_tests`; cast to
/// `Arc<dyn Store>` for the `for_tenant` scoping helper.
pub async fn open_postgres(url: &str) -> Result<Arc<postgres::PostgresStore>, StoreError> {
    Ok(Arc::new(postgres::PostgresStore::open(url, false).await?))
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
pub async fn open_backends(data_path: &Path, multi_tenant: bool) -> Result<Backends, StoreError> {
    let (store, embedded_sink): (Arc<dyn Store>, Arc<dyn AnalyticsSink>) =
        match std::env::var("QUARK_DATABASE_URL") {
            Ok(url) => {
                // Optional read replica: reads go to it, writes stay on the
                // primary. Unset or empty means both pools point at the primary
                // (behavior identical to today).
                let replica = std::env::var("QUARK_REPLICA_DATABASE_URL")
                    .ok()
                    .filter(|s| !s.is_empty());
                let pg = match replica {
                    Some(replica_url) => {
                        eprintln!("store: Postgres primary + read replica");
                        Arc::new(
                            postgres::PostgresStore::open_with_replica(
                                &url,
                                &replica_url,
                                multi_tenant,
                            )
                            .await?,
                        )
                    }
                    None => {
                        eprintln!("store: Postgres (single URL, no read replica)");
                        Arc::new(postgres::PostgresStore::open(&url, multi_tenant).await?)
                    }
                };
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
mod scoped_tests {
    use super::*;
    use crate::tenant::TenantId;

    fn test_rec(url: &str) -> Record {
        Record {
            url: url.into(),
            expiry: None,
            created: 0,
            tags: Vec::new(),
            max_visits: None,
            rules: Vec::new(),
            variants: Vec::new(),
            app_ios: None,
            app_android: None,
            folder: None,
            fallback_url: None,
            password_hash: None,
            tenant_id: crate::tenant::DEFAULT_TENANT,
        }
    }

    #[tokio::test]
    async fn scoped_store_isolates_links_by_tenant() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path()).await.unwrap();
        let a = store.clone().for_tenant(TenantId(1));
        let b = store.clone().for_tenant(TenantId(2));

        let rec = test_rec("https://example.com");
        a.put_link(100, &rec).await.unwrap();

        assert!(a.get_link(100).await.unwrap().is_some());
        // Tenant 2 must NOT see tenant 1's link at the same id.
        assert!(b.get_link(100).await.unwrap().is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_folder, normalize_tags, Record};

    #[test]
    fn normalize_folder_trims_and_preserves_case() {
        assert_eq!(
            normalize_folder(Some("  Marketing  ".into())),
            Some("Marketing".to_string())
        );
    }

    #[test]
    fn normalize_folder_empty_becomes_none() {
        assert_eq!(normalize_folder(Some("   ".into())), None);
        assert_eq!(normalize_folder(Some(String::new())), None);
        assert_eq!(normalize_folder(None), None);
    }

    #[test]
    fn normalize_folder_caps_length_at_48_chars() {
        let long = "a".repeat(60);
        let out = normalize_folder(Some(long)).unwrap();
        assert_eq!(out.chars().count(), 48);
    }

    #[test]
    fn record_without_folder_field_deserializes_to_none() {
        let old_blob = r#"{"url":"https://example.com","expiry":null,"created":1}"#;
        let rec: Record = serde_json::from_str(old_blob).unwrap();
        assert_eq!(rec.folder, None);
    }

    #[test]
    fn record_without_fallback_url_field_deserializes_to_none() {
        let old_blob = r#"{"url":"https://example.com","expiry":null,"created":1}"#;
        let rec: Record = serde_json::from_str(old_blob).unwrap();
        assert_eq!(rec.fallback_url, None);
    }

    #[test]
    fn record_round_trips_fallback_url() {
        let rec = Record {
            url: "https://example.com".into(),
            expiry: Some(10),
            created: 1,
            tags: vec![],
            max_visits: None,
            rules: vec![],
            variants: vec![],
            app_ios: None,
            app_android: None,
            folder: None,
            fallback_url: Some("https://example.com/ended".into()),
            password_hash: None,
            tenant_id: crate::tenant::DEFAULT_TENANT,
        };
        let json = serde_json::to_string(&rec).unwrap();
        let back: Record = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back.fallback_url.as_deref(),
            Some("https://example.com/ended")
        );
    }

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
            folder: None,
            fallback_url: None,
            password_hash: None,
            tenant_id: crate::tenant::DEFAULT_TENANT,
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
