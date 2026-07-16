use crate::analytics::{is_bot, Aggregates, AnalyticsSink, ClickEvent, Stats, EVENTS_MAX};
use crate::auth::ApiToken;
use crate::pixel::PixelConfig;
use crate::store::{LinkHealth, OutboxDelivery, OutboxRow, Record, Store, StoreError};
use crate::tenant::{Membership, Tenant, TenantId, User, DEFAULT_TENANT};
use crate::webhooks::WebhookSubscription;
use heed::byteorder::BigEndian;
use heed::types::{Bytes, Str, U64};
use heed::{Database, Env, EnvOpenOptions};
use std::collections::BTreeMap;
use std::path::Path;

type BeU64 = U64<BigEndian>;

/// Prefixes a big-endian tenant id onto a key so each tenant occupies a
/// disjoint, contiguous range within a shared sub-db. `tenant_id` is an
/// ownership prefix, never a code-space partition (the id/short-code namespace
/// stays global — see the `meta` counters, which are NOT prefixed).
fn tkey(tenant: TenantId, key: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + key.len());
    out.extend_from_slice(&tenant.0.to_be_bytes());
    out.extend_from_slice(key);
    out
}

/// `tkey` for a numeric (u64, big-endian) key — links/health/pixels/etc.
fn tkey_id(tenant: TenantId, id: u64) -> Vec<u8> {
    tkey(tenant, &id.to_be_bytes())
}

/// The 8-byte prefix identifying a tenant's contiguous key range.
fn tprefix(tenant: TenantId) -> [u8; 8] {
    tenant.0.to_be_bytes()
}

/// Decodes the `u64` id stored in the low 8 bytes of a tenant-prefixed key.
fn id_from_tkey(key: &[u8]) -> u64 {
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&key[8..16]);
    u64::from_be_bytes(buf)
}

/// Membership key: `user_id` (be) then `tenant_id` (be), so all of a user's
/// memberships share a contiguous `user_id` prefix.
fn membership_key(user_id: u64, tenant: TenantId) -> Vec<u8> {
    let mut out = Vec::with_capacity(16);
    out.extend_from_slice(&user_id.to_be_bytes());
    out.extend_from_slice(&tenant.0.to_be_bytes());
    out
}

/// The named tenant-owned sub-dbs re-keyed by the boot migration. `meta`
/// (global counters) and `sessions` (global hash lookup) are intentionally
/// absent — they are never tenant-prefixed.
const TENANT_OWNED_DBS: [&str; 11] = [
    "links",
    "aliases",
    "stats",
    "events",
    "webhooks",
    "api_tokens",
    "visits",
    "pixels",
    "wellknown",
    "health",
    "sheets",
];

/// Defensive partitioning of the 40-bit space across nodes (see docs/SCALING.md).
/// The 8 high bits identify the node; the 32 low bits are the node's local counter.
const NODE_BITS: u32 = 8;
const LOCAL_BITS: u32 = 40 - NODE_BITS;
const LOCAL_MAX: u64 = (1u64 << LOCAL_BITS) - 1;

/// Number of named LMDB sub-databases opened in the environment.
/// 13 original + 3 identity sub-dbs (`tenants`, `users`, `memberships`).
const MAX_DBS: u32 = 16;
/// Virtual address space (mmap) reserved for the LMDB environment.
const MAP_SIZE_BYTES: usize = 64 * 1024 * 1024 * 1024;

/// Reads `QUARK_NODE_ID`: absent/empty -> `None` (single-node mode, full 40 bits);
/// "0".."255" -> `Some(n)`; anything else -> error (fail-fast on startup).
fn parse_node_id(raw: Option<String>) -> Result<Option<u8>, StoreError> {
    match raw.as_deref() {
        None | Some("") => Ok(None),
        Some(s) => s.parse::<u8>().map(Some).map_err(|_| {
            StoreError::Backend(format!("invalid QUARK_NODE_ID: {s} (expected 0-255)"))
        }),
    }
}

/// Composes the final id from the counter. Without node-id: identity (the caller
/// applies the `permute::MAX_ID` cap, as today). With node-id: prefixes the high
/// bits and fails BEFORE overflowing the local range (no-leak invariant).
fn compose_id(node_id: Option<u8>, counter: u64) -> Result<u64, StoreError> {
    match node_id {
        None => Ok(counter),
        Some(node) => {
            if counter > LOCAL_MAX {
                Err(StoreError::IdSpaceExhausted)
            } else {
                Ok(((node as u64) << LOCAL_BITS) | counter)
            }
        }
    }
}

pub struct LmdbStore {
    env: Env,
    // Tenant-owned sub-dbs: keys are `tkey(tenant, original_key)` (Bytes), so
    // each tenant occupies a disjoint contiguous range.
    links: Database<Bytes, Bytes>,
    aliases: Database<Bytes, BeU64>,
    // Global counters (id/short-code namespace is global) — NOT tenant-prefixed.
    meta: Database<Str, BeU64>,
    stats: Database<Bytes, Bytes>,
    events: Database<Bytes, Bytes>,
    webhooks: Database<Bytes, Bytes>,
    api_tokens: Database<Bytes, Bytes>,
    visits: Database<Bytes, BeU64>,
    pixels: Database<Bytes, Bytes>,
    wellknown: Database<Bytes, Str>,
    health: Database<Bytes, Bytes>,
    // Global hash-lookup — keyed by token hash, tenant travels in the value.
    sessions: Database<Str, Bytes>,
    sheets: Database<Bytes, Bytes>,
    // Identity / tenancy (tenant-less).
    tenants: Database<BeU64, Bytes>,
    users: Database<Str, Bytes>,
    memberships: Database<Bytes, Bytes>,
    node_id: Option<u8>,
}

impl LmdbStore {
    /// Opens reading `QUARK_NODE_ID` from the environment (fail-fast if invalid).
    pub fn open(path: &Path) -> Result<LmdbStore, StoreError> {
        let node_id = parse_node_id(std::env::var("QUARK_NODE_ID").ok())?;
        LmdbStore::open_with_node_id(path, node_id)
    }

    /// Opens with an explicit node-id (used by tests; avoids a global env race).
    pub fn open_with_node_id(path: &Path, node_id: Option<u8>) -> Result<LmdbStore, StoreError> {
        std::fs::create_dir_all(path).map_err(heed::Error::Io)?;
        let env = unsafe {
            EnvOpenOptions::new()
                .map_size(MAP_SIZE_BYTES)
                .max_dbs(MAX_DBS)
                .open(path)?
        };
        let mut wtxn = env.write_txn()?;
        let links = env.create_database(&mut wtxn, Some("links"))?;
        let aliases = env.create_database(&mut wtxn, Some("aliases"))?;
        let meta = env.create_database(&mut wtxn, Some("meta"))?;
        let stats = env.create_database(&mut wtxn, Some("stats"))?;
        let events = env.create_database(&mut wtxn, Some("events"))?;
        let webhooks = env.create_database(&mut wtxn, Some("webhooks"))?;
        let api_tokens = env.create_database(&mut wtxn, Some("api_tokens"))?;
        let visits = env.create_database(&mut wtxn, Some("visits"))?;
        let pixels = env.create_database(&mut wtxn, Some("pixels"))?;
        let wellknown = env.create_database(&mut wtxn, Some("wellknown"))?;
        let health = env.create_database(&mut wtxn, Some("health"))?;
        let sessions = env.create_database(&mut wtxn, Some("sessions"))?;
        let sheets = env.create_database(&mut wtxn, Some("sheets"))?;
        let tenants = env.create_database(&mut wtxn, Some("tenants"))?;
        let users = env.create_database(&mut wtxn, Some("users"))?;
        let memberships = env.create_database(&mut wtxn, Some("memberships"))?;
        wtxn.commit()?;
        let store = LmdbStore {
            env,
            links,
            aliases,
            meta,
            stats,
            events,
            webhooks,
            api_tokens,
            visits,
            pixels,
            wellknown,
            health,
            sessions,
            sheets,
            tenants,
            users,
            memberships,
            node_id,
        };
        // One-time re-keying of any pre-tenancy data into DEFAULT_TENANT, then
        // seed the default tenant row. Both are idempotent (guarded / upsert).
        store.migrate_pre_tenancy_keys_to_default()?;
        store.seed_default_tenant()?;
        Ok(store)
    }

    /// One-time migration: any tenant-owned key written before tenancy has no
    /// 8-byte prefix. On boot, re-key every such entry under `DEFAULT_TENANT`.
    /// Guarded by a `meta["tenancy_migrated"]` marker so it runs at most once;
    /// on a fresh DB it re-keys nothing and just sets the marker.
    fn migrate_pre_tenancy_keys_to_default(&self) -> Result<(), StoreError> {
        let mut wtxn = self.env.write_txn()?;
        if self.meta.get(&wtxn, "tenancy_migrated")?.unwrap_or(0) == 1 {
            return Ok(());
        }
        let prefix = tprefix(DEFAULT_TENANT);
        for name in TENANT_OWNED_DBS {
            // Re-open each sub-db with a raw Bytes/Bytes codec so we can re-key
            // regardless of the value type it normally stores.
            let db: Database<Bytes, Bytes> = self
                .env
                .open_database(&wtxn, Some(name))?
                .expect("sub-db created in open()");
            let entries: Vec<(Vec<u8>, Vec<u8>)> = {
                let mut v = Vec::new();
                for item in db.iter(&wtxn)? {
                    let (k, val) = item?;
                    v.push((k.to_vec(), val.to_vec()));
                }
                v
            };
            db.clear(&mut wtxn)?;
            for (k, val) in entries {
                let mut nk = Vec::with_capacity(8 + k.len());
                nk.extend_from_slice(&prefix);
                nk.extend_from_slice(&k);
                db.put(&mut wtxn, &nk, &val)?;
            }
        }
        self.meta.put(&mut wtxn, "tenancy_migrated", &1)?;
        wtxn.commit()?;
        Ok(())
    }

    /// Upserts the `DEFAULT_TENANT` row (id 0, slug "default"), so the default
    /// tenant always exists. Idempotent.
    fn seed_default_tenant(&self) -> Result<(), StoreError> {
        let mut wtxn = self.env.write_txn()?;
        if self.tenants.get(&wtxn, &DEFAULT_TENANT.0)?.is_none() {
            let t = Tenant {
                id: DEFAULT_TENANT,
                name: "default".into(),
                slug: "default".into(),
                created: 0,
            };
            self.tenants
                .put(&mut wtxn, &DEFAULT_TENANT.0, &serde_json::to_vec(&t)?)?;
        }
        wtxn.commit()?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl Store for LmdbStore {
    async fn next_id(&self, _tenant: TenantId) -> Result<u64, StoreError> {
        // The id / short-code namespace is GLOBAL by design: the `meta` counter
        // is never tenant-prefixed. `tenant` is accepted for a uniform trait but
        // does not partition the counter.
        let mut wtxn = self.env.write_txn()?;
        let cur = self.meta.get(&wtxn, "next_id")?.unwrap_or(0);
        let next = cur + 1;
        let id = compose_id(self.node_id, next)?;
        self.meta.put(&mut wtxn, "next_id", &next)?;
        wtxn.commit()?;
        Ok(id)
    }

    async fn get_link(&self, tenant: TenantId, id: u64) -> Result<Option<Record>, StoreError> {
        let rtxn = self.env.read_txn()?;
        match self.links.get(&rtxn, &tkey_id(tenant, id))? {
            Some(bytes) => Ok(Some(serde_json::from_slice(bytes)?)),
            None => Ok(None),
        }
    }

    async fn put_link(&self, tenant: TenantId, id: u64, rec: &Record) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec(rec)?;
        let mut wtxn = self.env.write_txn()?;
        self.links.put(&mut wtxn, &tkey_id(tenant, id), &bytes)?;
        wtxn.commit()?;
        Ok(())
    }

    async fn get_alias(&self, tenant: TenantId, alias: &str) -> Result<Option<u64>, StoreError> {
        let rtxn = self.env.read_txn()?;
        Ok(self.aliases.get(&rtxn, &tkey(tenant, alias.as_bytes()))?)
    }

    /// Writes alias + link in a single transaction: either both or neither.
    /// Avoids an orphaned link if the process fails between the two writes.
    async fn put_alias_and_link(
        &self,
        tenant: TenantId,
        alias: &str,
        id: u64,
        rec: &Record,
    ) -> Result<bool, StoreError> {
        let bytes = serde_json::to_vec(rec)?;
        let mut wtxn = self.env.write_txn()?;
        let akey = tkey(tenant, alias.as_bytes());
        if self.aliases.get(&wtxn, &akey)?.is_some() {
            return Ok(false);
        }
        self.links.put(&mut wtxn, &tkey_id(tenant, id), &bytes)?;
        self.aliases.put(&mut wtxn, &akey, &id)?;
        wtxn.commit()?;
        Ok(true)
    }

    // The `_tx` variants exist to make the link mutation atomic with the
    // webhook-outbox enqueue on Postgres. On the single-node LMDB backend
    // lifecycle events ride the in-memory channel and `deliveries` is always
    // empty, so these delegate to the plain op and ignore it (single-node
    // behavior is unchanged).
    async fn put_link_tx(
        &self,
        tenant: TenantId,
        id: u64,
        rec: &Record,
        _deliveries: &[OutboxRow],
    ) -> Result<(), StoreError> {
        self.put_link(tenant, id, rec).await
    }

    async fn put_alias_and_link_tx(
        &self,
        tenant: TenantId,
        alias: &str,
        id: u64,
        rec: &Record,
        _deliveries: &[OutboxRow],
    ) -> Result<bool, StoreError> {
        self.put_alias_and_link(tenant, alias, id, rec).await
    }

    async fn delete_link_tx(
        &self,
        tenant: TenantId,
        id: u64,
        _deliveries: &[OutboxRow],
    ) -> Result<(), StoreError> {
        self.delete_link(tenant, id).await
    }

    async fn list_links(
        &self,
        tenant: TenantId,
        after: Option<u64>,
        limit: usize,
        tag: Option<&str>,
        folder: Option<&str>,
    ) -> Result<Vec<(u64, Record)>, StoreError> {
        let rtxn = self.env.read_txn()?;
        let prefix = tprefix(tenant);
        let mut out = Vec::new();
        // Iterate only this tenant's contiguous prefix range (keyset by id).
        for item in self.links.prefix_iter(&rtxn, &prefix)? {
            let (key, bytes) = item?;
            let id = id_from_tkey(key);
            if let Some(a) = after {
                if id <= a {
                    continue;
                }
            }
            let rec: Record = serde_json::from_slice(bytes)?;
            if let Some(t) = tag {
                if !rec.tags.iter().any(|x| x == t) {
                    continue;
                }
            }
            if let Some(f) = folder {
                match &rec.folder {
                    Some(rf) if rf.to_lowercase() == f.to_lowercase() => {}
                    _ => continue,
                }
            }
            out.push((id, rec));
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    async fn search_links(
        &self,
        _tenant: TenantId,
        _q: &str,
        _after: Option<u64>,
        _limit: usize,
        _tag: Option<&str>,
        _folder: Option<&str>,
    ) -> Result<Vec<(u64, Record)>, StoreError> {
        Err(StoreError::Unsupported)
    }

    async fn list_aliases(&self, tenant: TenantId) -> Result<Vec<(String, u64)>, StoreError> {
        let rtxn = self.env.read_txn()?;
        let prefix = tprefix(tenant);
        let mut out = Vec::new();
        for item in self.aliases.prefix_iter(&rtxn, &prefix)? {
            let (key, id) = item?;
            // Strip the 8-byte tenant prefix to recover the alias string.
            let alias = String::from_utf8_lossy(&key[8..]).into_owned();
            out.push((alias, id));
        }
        Ok(out)
    }

    async fn delete_link(&self, tenant: TenantId, id: u64) -> Result<(), StoreError> {
        let mut wtxn = self.env.write_txn()?;
        let k = tkey_id(tenant, id);
        self.links.delete(&mut wtxn, &k)?;
        // Drop the link's health entry too, so deleted links don't leave orphan
        // rows that grow unbounded and slow the admin list / sweep.
        self.health.delete(&mut wtxn, &k)?;
        wtxn.commit()?;
        Ok(())
    }

    async fn delete_alias(&self, tenant: TenantId, alias: &str) -> Result<(), StoreError> {
        let mut wtxn = self.env.write_txn()?;
        self.aliases
            .delete(&mut wtxn, &tkey(tenant, alias.as_bytes()))?;
        wtxn.commit()?;
        Ok(())
    }

    async fn list_webhooks(
        &self,
        tenant: TenantId,
    ) -> Result<Vec<WebhookSubscription>, StoreError> {
        let rtxn = self.env.read_txn()?;
        let mut out = Vec::new();
        for item in self.webhooks.prefix_iter(&rtxn, &tprefix(tenant))? {
            let (_, bytes) = item?;
            out.push(serde_json::from_slice(bytes)?);
        }
        Ok(out)
    }

    async fn get_webhook(
        &self,
        tenant: TenantId,
        id: u64,
    ) -> Result<Option<WebhookSubscription>, StoreError> {
        let rtxn = self.env.read_txn()?;
        match self.webhooks.get(&rtxn, &tkey_id(tenant, id))? {
            Some(bytes) => Ok(Some(serde_json::from_slice(bytes)?)),
            None => Ok(None),
        }
    }

    async fn put_webhook(
        &self,
        tenant: TenantId,
        sub: &WebhookSubscription,
    ) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec(sub)?;
        let mut wtxn = self.env.write_txn()?;
        self.webhooks
            .put(&mut wtxn, &tkey_id(tenant, sub.id), &bytes)?;
        wtxn.commit()?;
        Ok(())
    }

    async fn delete_webhook(&self, tenant: TenantId, id: u64) -> Result<bool, StoreError> {
        let mut wtxn = self.env.write_txn()?;
        let existed = self.webhooks.delete(&mut wtxn, &tkey_id(tenant, id))?;
        wtxn.commit()?;
        Ok(existed)
    }

    async fn next_webhook_id(&self, _tenant: TenantId) -> Result<u64, StoreError> {
        // Global id namespace (not tenant-prefixed).
        let mut wtxn = self.env.write_txn()?;
        let cur = self.meta.get(&wtxn, "next_webhook_id")?.unwrap_or(0);
        let next = cur + 1;
        self.meta.put(&mut wtxn, "next_webhook_id", &next)?;
        wtxn.commit()?;
        Ok(next)
    }

    async fn bump_visits(&self, tenant: TenantId, id: u64) -> Result<u64, StoreError> {
        let mut wtxn = self.env.write_txn()?;
        let k = tkey_id(tenant, id);
        let cur = self.visits.get(&wtxn, &k)?.unwrap_or(0);
        let next = cur + 1;
        self.visits.put(&mut wtxn, &k, &next)?;
        wtxn.commit()?;
        Ok(next)
    }

    async fn put_link_health(
        &self,
        tenant: TenantId,
        id: u64,
        health: &LinkHealth,
    ) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec(health)?;
        let mut wtxn = self.env.write_txn()?;
        self.health.put(&mut wtxn, &tkey_id(tenant, id), &bytes)?;
        wtxn.commit()?;
        Ok(())
    }

    async fn list_link_health(
        &self,
        tenant: TenantId,
    ) -> Result<Vec<(u64, LinkHealth)>, StoreError> {
        let rtxn = self.env.read_txn()?;
        let mut out = Vec::new();
        for item in self.health.prefix_iter(&rtxn, &tprefix(tenant))? {
            let (key, bytes) = item?;
            out.push((id_from_tkey(key), serde_json::from_slice(bytes)?));
        }
        Ok(out)
    }

    async fn link_health_for(
        &self,
        tenant: TenantId,
        ids: &[u64],
    ) -> Result<Vec<(u64, LinkHealth)>, StoreError> {
        let rtxn = self.env.read_txn()?;
        let mut out = Vec::with_capacity(ids.len());
        for &id in ids {
            if let Some(bytes) = self.health.get(&rtxn, &tkey_id(tenant, id))? {
                out.push((id, serde_json::from_slice(bytes)?));
            }
        }
        Ok(out)
    }

    async fn list_broken_link_ids(&self, tenant: TenantId) -> Result<Vec<u64>, StoreError> {
        let rtxn = self.env.read_txn()?;
        let mut out = Vec::new();
        for item in self.health.prefix_iter(&rtxn, &tprefix(tenant))? {
            let (key, bytes) = item?;
            let h: LinkHealth = serde_json::from_slice(bytes)?;
            if !h.healthy {
                out.push(id_from_tkey(key));
            }
        }
        out.sort_unstable();
        Ok(out)
    }

    async fn try_acquire_health_lease(
        &self,
        _holder: &str,
        _ttl_secs: u64,
    ) -> Result<bool, StoreError> {
        // LMDB is single-node: there is only ever one checker.
        Ok(true)
    }

    async fn put_sheets_connection(
        &self,
        tenant: TenantId,
        c: &crate::sheets::SheetsConnection,
    ) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec(c)?;
        let mut wtxn = self.env.write_txn()?;
        self.sheets
            .put(&mut wtxn, &tkey(tenant, b"connection"), &bytes)?;
        wtxn.commit()?;
        Ok(())
    }

    async fn get_sheets_connection(
        &self,
        tenant: TenantId,
    ) -> Result<Option<crate::sheets::SheetsConnection>, StoreError> {
        let rtxn = self.env.read_txn()?;
        match self.sheets.get(&rtxn, &tkey(tenant, b"connection"))? {
            Some(bytes) => Ok(Some(serde_json::from_slice(bytes)?)),
            None => Ok(None),
        }
    }

    async fn delete_sheets_connection(&self, tenant: TenantId) -> Result<(), StoreError> {
        let mut wtxn = self.env.write_txn()?;
        self.sheets
            .delete(&mut wtxn, &tkey(tenant, b"connection"))?;
        wtxn.commit()?;
        Ok(())
    }

    async fn try_acquire_sheets_lease(
        &self,
        _holder: &str,
        _ttl_secs: u64,
    ) -> Result<bool, StoreError> {
        // LMDB is single-node: there is only ever one scheduled sync.
        Ok(true)
    }

    async fn put_session(
        &self,
        _tenant: TenantId,
        session: &crate::auth::Session,
    ) -> Result<(), StoreError> {
        // Sessions are looked up by token hash globally; the hash is unique
        // across tenants. In P1a the `Session` struct does not yet carry the
        // tenant (that field lands in P1b), so `tenant` is accepted but not
        // persisted here — this keeps P1a a pure data-layer change.
        let bytes = serde_json::to_vec(session)?;
        let mut wtxn = self.env.write_txn()?;
        self.sessions.put(&mut wtxn, &session.token_hash, &bytes)?;
        wtxn.commit()?;
        Ok(())
    }

    async fn get_session_by_hash(
        &self,
        token_hash: &str,
        now: u64,
    ) -> Result<Option<crate::auth::Session>, StoreError> {
        let rtxn = self.env.read_txn()?;
        match self.sessions.get(&rtxn, token_hash)? {
            Some(bytes) => {
                let s: crate::auth::Session = serde_json::from_slice(bytes)?;
                Ok(if s.expires <= now { None } else { Some(s) })
            }
            None => Ok(None),
        }
    }

    async fn delete_session(&self, token_hash: &str) -> Result<(), StoreError> {
        let mut wtxn = self.env.write_txn()?;
        self.sessions.delete(&mut wtxn, token_hash)?;
        wtxn.commit()?;
        Ok(())
    }

    async fn gc_sessions(&self, now: u64) -> Result<(), StoreError> {
        let mut expired: Vec<String> = Vec::new();
        {
            let rtxn = self.env.read_txn()?;
            for item in self.sessions.iter(&rtxn)? {
                let (hash, bytes) = item?;
                let s: crate::auth::Session = serde_json::from_slice(bytes)?;
                if s.expires <= now {
                    expired.push(hash.to_string());
                }
            }
        }
        if !expired.is_empty() {
            let mut wtxn = self.env.write_txn()?;
            for hash in &expired {
                self.sessions.delete(&mut wtxn, hash)?;
            }
            wtxn.commit()?;
        }
        Ok(())
    }

    async fn list_tags(&self, tenant: TenantId) -> Result<Vec<(String, u64)>, StoreError> {
        let rtxn = self.env.read_txn()?;
        let mut counts: BTreeMap<String, u64> = BTreeMap::new();
        for item in self.links.prefix_iter(&rtxn, &tprefix(tenant))? {
            let (_, bytes) = item?;
            let rec: Record = serde_json::from_slice(bytes)?;
            let mut seen = std::collections::BTreeSet::new();
            for t in rec.tags {
                if seen.insert(t.clone()) {
                    *counts.entry(t).or_insert(0) += 1;
                }
            }
        }
        Ok(counts.into_iter().collect())
    }

    async fn list_folders(&self, tenant: TenantId) -> Result<Vec<(String, u64)>, StoreError> {
        let rtxn = self.env.read_txn()?;
        let mut counts: BTreeMap<String, u64> = BTreeMap::new();
        for item in self.links.prefix_iter(&rtxn, &tprefix(tenant))? {
            let (_, bytes) = item?;
            let rec: Record = serde_json::from_slice(bytes)?;
            if let Some(f) = rec.folder {
                *counts.entry(f).or_insert(0) += 1;
            }
        }
        Ok(counts.into_iter().collect())
    }

    async fn list_api_tokens(&self, tenant: TenantId) -> Result<Vec<ApiToken>, StoreError> {
        let rtxn = self.env.read_txn()?;
        let mut out = Vec::new();
        for item in self.api_tokens.prefix_iter(&rtxn, &tprefix(tenant))? {
            let (_, bytes) = item?;
            out.push(serde_json::from_slice(bytes)?);
        }
        Ok(out)
    }

    /// LMDB has no secondary index, so the hot-path lookup by hash scans the
    /// (small, admin-managed) token set across ALL tenants (the hash is globally
    /// unique, so the match is unambiguous). Postgres backs this with a real
    /// index for the network-backend case.
    async fn get_api_token_by_hash(&self, hash: &str) -> Result<Option<ApiToken>, StoreError> {
        let rtxn = self.env.read_txn()?;
        for item in self.api_tokens.iter(&rtxn)? {
            let (_, bytes) = item?;
            let token: ApiToken = serde_json::from_slice(bytes)?;
            if token.token_hash == hash {
                return Ok(Some(token));
            }
        }
        Ok(None)
    }

    async fn put_api_token(&self, tenant: TenantId, token: &ApiToken) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec(token)?;
        let mut wtxn = self.env.write_txn()?;
        self.api_tokens
            .put(&mut wtxn, &tkey_id(tenant, token.id), &bytes)?;
        wtxn.commit()?;
        Ok(())
    }

    async fn delete_api_token(&self, tenant: TenantId, id: u64) -> Result<bool, StoreError> {
        let mut wtxn = self.env.write_txn()?;
        let existed = self.api_tokens.delete(&mut wtxn, &tkey_id(tenant, id))?;
        wtxn.commit()?;
        Ok(existed)
    }

    async fn next_api_token_id(&self, _tenant: TenantId) -> Result<u64, StoreError> {
        // Global id namespace (not tenant-prefixed).
        let mut wtxn = self.env.write_txn()?;
        let cur = self.meta.get(&wtxn, "next_api_token_id")?.unwrap_or(0);
        let next = cur + 1;
        self.meta.put(&mut wtxn, "next_api_token_id", &next)?;
        wtxn.commit()?;
        Ok(next)
    }

    async fn visits(&self, tenant: TenantId, id: u64) -> Result<u64, StoreError> {
        let rtxn = self.env.read_txn()?;
        Ok(self.visits.get(&rtxn, &tkey_id(tenant, id))?.unwrap_or(0))
    }

    async fn next_pixel_id(&self, _tenant: TenantId) -> Result<u64, StoreError> {
        // Global id namespace (not tenant-prefixed).
        let mut wtxn = self.env.write_txn()?;
        let cur = self.meta.get(&wtxn, "next_pixel_id")?.unwrap_or(0);
        let next = cur + 1;
        self.meta.put(&mut wtxn, "next_pixel_id", &next)?;
        wtxn.commit()?;
        Ok(next)
    }

    async fn get_pixel(
        &self,
        tenant: TenantId,
        id: u64,
    ) -> Result<Option<PixelConfig>, StoreError> {
        let rtxn = self.env.read_txn()?;
        match self.pixels.get(&rtxn, &tkey_id(tenant, id))? {
            Some(bytes) => Ok(Some(serde_json::from_slice(bytes)?)),
            None => Ok(None),
        }
    }

    async fn put_pixel(&self, tenant: TenantId, config: &PixelConfig) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec(config)?;
        let mut wtxn = self.env.write_txn()?;
        self.pixels
            .put(&mut wtxn, &tkey_id(tenant, config.id), &bytes)?;
        wtxn.commit()?;
        Ok(())
    }

    async fn delete_pixel(&self, tenant: TenantId, id: u64) -> Result<bool, StoreError> {
        let mut wtxn = self.env.write_txn()?;
        let existed = self.pixels.delete(&mut wtxn, &tkey_id(tenant, id))?;
        wtxn.commit()?;
        Ok(existed)
    }

    async fn list_pixels(&self, tenant: TenantId) -> Result<Vec<PixelConfig>, StoreError> {
        let rtxn = self.env.read_txn()?;
        let mut out = Vec::new();
        for item in self.pixels.prefix_iter(&rtxn, &tprefix(tenant))? {
            let (_, bytes) = item?;
            out.push(serde_json::from_slice(bytes)?);
        }
        Ok(out)
    }

    async fn get_wellknown(
        &self,
        tenant: TenantId,
        name: &str,
    ) -> Result<Option<String>, StoreError> {
        let rtxn = self.env.read_txn()?;
        Ok(self
            .wellknown
            .get(&rtxn, &tkey(tenant, name.as_bytes()))?
            .map(|s| s.to_string()))
    }

    async fn put_wellknown(
        &self,
        tenant: TenantId,
        name: &str,
        body: &str,
    ) -> Result<(), StoreError> {
        let mut wtxn = self.env.write_txn()?;
        self.wellknown
            .put(&mut wtxn, &tkey(tenant, name.as_bytes()), body)?;
        wtxn.commit()?;
        Ok(())
    }

    async fn delete_wellknown(&self, tenant: TenantId, name: &str) -> Result<(), StoreError> {
        let mut wtxn = self.env.write_txn()?;
        self.wellknown
            .delete(&mut wtxn, &tkey(tenant, name.as_bytes()))?;
        wtxn.commit()?;
        Ok(())
    }

    // --- Identity / tenancy ---
    async fn put_tenant(&self, t: &Tenant) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec(t)?;
        let mut wtxn = self.env.write_txn()?;
        self.tenants.put(&mut wtxn, &t.id.0, &bytes)?;
        wtxn.commit()?;
        Ok(())
    }

    async fn get_tenant(&self, id: TenantId) -> Result<Option<Tenant>, StoreError> {
        let rtxn = self.env.read_txn()?;
        match self.tenants.get(&rtxn, &id.0)? {
            Some(bytes) => Ok(Some(serde_json::from_slice(bytes)?)),
            None => Ok(None),
        }
    }

    async fn next_user_id(&self) -> Result<u64, StoreError> {
        let mut wtxn = self.env.write_txn()?;
        let cur = self.meta.get(&wtxn, "next_user_id")?.unwrap_or(0);
        let next = cur + 1;
        self.meta.put(&mut wtxn, "next_user_id", &next)?;
        wtxn.commit()?;
        Ok(next)
    }

    async fn next_tenant_id(&self) -> Result<u64, StoreError> {
        let mut wtxn = self.env.write_txn()?;
        let cur = self.meta.get(&wtxn, "next_tenant_id")?.unwrap_or(0);
        let next = cur + 1;
        self.meta.put(&mut wtxn, "next_tenant_id", &next)?;
        wtxn.commit()?;
        Ok(next)
    }

    async fn put_user(&self, u: &User) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec(u)?;
        let mut wtxn = self.env.write_txn()?;
        self.users.put(&mut wtxn, &u.subject, &bytes)?;
        wtxn.commit()?;
        Ok(())
    }

    async fn get_user_by_subject(&self, subject: &str) -> Result<Option<User>, StoreError> {
        let rtxn = self.env.read_txn()?;
        match self.users.get(&rtxn, subject)? {
            Some(bytes) => Ok(Some(serde_json::from_slice(bytes)?)),
            None => Ok(None),
        }
    }

    async fn put_membership(&self, m: &Membership) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec(m)?;
        let mut wtxn = self.env.write_txn()?;
        self.memberships
            .put(&mut wtxn, &membership_key(m.user_id, m.tenant_id), &bytes)?;
        wtxn.commit()?;
        Ok(())
    }

    async fn get_membership(
        &self,
        user_id: u64,
        tenant: TenantId,
    ) -> Result<Option<Membership>, StoreError> {
        let rtxn = self.env.read_txn()?;
        match self
            .memberships
            .get(&rtxn, &membership_key(user_id, tenant))?
        {
            Some(bytes) => Ok(Some(serde_json::from_slice(bytes)?)),
            None => Ok(None),
        }
    }

    async fn list_memberships_for_user(&self, user_id: u64) -> Result<Vec<Membership>, StoreError> {
        let rtxn = self.env.read_txn()?;
        let mut out = Vec::new();
        for item in self
            .memberships
            .prefix_iter(&rtxn, &user_id.to_be_bytes())?
        {
            let (_, bytes) = item?;
            out.push(serde_json::from_slice(bytes)?);
        }
        Ok(out)
    }

    // The durable webhook outbox is Postgres-only. On the single-node LMDB
    // backend every event (lifecycle and clicked) rides the in-memory
    // best-effort channel, and `main.rs` never spawns the relay nor routes
    // lifecycle events to the outbox here, so these are never invoked. They
    // are implemented as no-ops (rather than `unimplemented!()`) to keep the
    // trait total and avoid any chance of a panic if wiring ever changes.
    async fn enqueue_deliveries(&self, _rows: &[OutboxRow]) -> Result<(), StoreError> {
        Ok(())
    }

    async fn claim_due_deliveries(
        &self,
        _now: u64,
        _limit: i64,
    ) -> Result<Vec<OutboxDelivery>, StoreError> {
        Ok(Vec::new())
    }

    async fn mark_delivered(&self, _id: i64) -> Result<(), StoreError> {
        Ok(())
    }

    async fn mark_retry(
        &self,
        _id: i64,
        _next_attempt_at: u64,
        _attempts: u32,
    ) -> Result<(), StoreError> {
        Ok(())
    }

    async fn mark_dead(&self, _id: i64, _attempts: u32) -> Result<(), StoreError> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl AnalyticsSink for LmdbStore {
    async fn record_batch(&self, events: &[ClickEvent]) -> Result<(), StoreError> {
        if events.is_empty() {
            return Ok(());
        }
        let mut by_id: BTreeMap<u64, Vec<&ClickEvent>> = BTreeMap::new();
        for e in events {
            by_id.entry(e.id).or_default().push(e);
        }
        // Analytics is single-tenant in P1a (per-tenant click routing is a later
        // task); stats/events keys are prefixed with DEFAULT_TENANT to stay
        // consistent with the tenant-prefixed sub-dbs and the boot migration.
        let mut wtxn = self.env.write_txn()?;
        for (id, evs) in by_id {
            let k = tkey_id(DEFAULT_TENANT, id);
            let mut agg: Aggregates = match self.stats.get(&wtxn, &k)? {
                Some(b) => serde_json::from_slice(b)?,
                None => Aggregates::default(),
            };
            for e in &evs {
                agg.apply(e);
            }
            self.stats.put(&mut wtxn, &k, &serde_json::to_vec(&agg)?)?;

            let mut recent: Vec<ClickEvent> = match self.events.get(&wtxn, &k)? {
                Some(b) => serde_json::from_slice(b)?,
                None => Vec::new(),
            };
            for e in &evs {
                recent.push((*e).clone());
            }
            if recent.len() > EVENTS_MAX {
                let drop = recent.len() - EVENTS_MAX;
                recent.drain(0..drop);
            }
            self.events
                .put(&mut wtxn, &k, &serde_json::to_vec(&recent)?)?;
        }
        wtxn.commit()?;
        Ok(())
    }

    async fn stats(&self, id: u64) -> Result<Option<Stats>, StoreError> {
        let rtxn = self.env.read_txn()?;
        let k = tkey_id(DEFAULT_TENANT, id);
        let agg = match self.stats.get(&rtxn, &k)? {
            Some(b) => serde_json::from_slice::<Aggregates>(b)?,
            None => return Ok(None),
        };
        let mut recent: Vec<ClickEvent> = match self.events.get(&rtxn, &k)? {
            Some(b) => serde_json::from_slice(b)?,
            None => Vec::new(),
        };
        for e in &mut recent {
            e.bot = is_bot(e.user_agent.as_deref());
        }
        Ok(Some(Stats {
            aggregates: agg,
            recent,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{compose_id, parse_node_id, LmdbStore, LOCAL_BITS, LOCAL_MAX};
    use crate::auth::{hash_token, ApiToken, Scope};
    use crate::pixel::{PixelConfig, PixelCredentials, Provider};
    use crate::store::{Record, Store, StoreError};
    use crate::webhooks::{EventType, WebhookSubscription};

    #[test]
    fn parse_node_id_absent_or_empty_becomes_none() {
        assert!(matches!(parse_node_id(None), Ok(None)));
        assert!(matches!(parse_node_id(Some(String::new())), Ok(None)));
    }

    #[test]
    fn parse_node_id_valid() {
        assert!(matches!(parse_node_id(Some("0".into())), Ok(Some(0))));
        assert!(matches!(parse_node_id(Some("255".into())), Ok(Some(255))));
        assert!(matches!(parse_node_id(Some("7".into())), Ok(Some(7))));
    }

    #[test]
    fn parse_node_id_invalid_errors() {
        assert!(matches!(
            parse_node_id(Some("256".into())),
            Err(StoreError::Backend(_))
        ));
        assert!(matches!(
            parse_node_id(Some("-1".into())),
            Err(StoreError::Backend(_))
        ));
        assert!(matches!(
            parse_node_id(Some("abc".into())),
            Err(StoreError::Backend(_))
        ));
    }

    #[test]
    fn compose_id_without_node_is_identity() {
        assert_eq!(compose_id(None, 1).unwrap(), 1);
        assert_eq!(compose_id(None, 1_000_000_000).unwrap(), 1_000_000_000);
    }

    #[test]
    fn compose_id_with_node_prefixes_high_bits() {
        assert_eq!(compose_id(Some(0), 1).unwrap(), 1);
        assert_eq!(compose_id(Some(1), 1).unwrap(), (1u64 << LOCAL_BITS) | 1);
        assert_eq!(compose_id(Some(5), 42).unwrap(), (5u64 << LOCAL_BITS) | 42);
    }

    #[test]
    fn compose_id_node_ranges_are_disjoint() {
        let node_0_max = compose_id(Some(0), LOCAL_MAX).unwrap();
        let node_1_min = compose_id(Some(1), 1).unwrap();
        assert!(node_0_max < node_1_min);
    }

    #[test]
    fn compose_id_local_counter_overflow_errors() {
        assert_eq!(
            compose_id(Some(3), LOCAL_MAX).unwrap(),
            (3u64 << LOCAL_BITS) | LOCAL_MAX
        );
        assert!(matches!(
            compose_id(Some(3), LOCAL_MAX + 1),
            Err(StoreError::IdSpaceExhausted)
        ));
    }

    #[tokio::test]
    async fn next_id_default_is_compatible_with_today() {
        let dir = tempfile::tempdir().unwrap();
        let s = LmdbStore::open_with_node_id(dir.path(), None).unwrap();
        assert_eq!(s.next_id(crate::tenant::DEFAULT_TENANT).await.unwrap(), 1);
        assert_eq!(s.next_id(crate::tenant::DEFAULT_TENANT).await.unwrap(), 2);
        assert_eq!(s.next_id(crate::tenant::DEFAULT_TENANT).await.unwrap(), 3);
    }

    #[tokio::test]
    async fn next_id_with_node_prefixes_and_increments_local() {
        let dir = tempfile::tempdir().unwrap();
        let s = LmdbStore::open_with_node_id(dir.path(), Some(5)).unwrap();
        assert_eq!(
            s.next_id(crate::tenant::DEFAULT_TENANT).await.unwrap(),
            (5u64 << LOCAL_BITS) | 1
        );
        assert_eq!(
            s.next_id(crate::tenant::DEFAULT_TENANT).await.unwrap(),
            (5u64 << LOCAL_BITS) | 2
        );
    }

    #[tokio::test]
    async fn next_id_from_distinct_nodes_does_not_collide() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let a = LmdbStore::open_with_node_id(dir_a.path(), Some(0)).unwrap();
        let b = LmdbStore::open_with_node_id(dir_b.path(), Some(1)).unwrap();
        assert_ne!(
            a.next_id(crate::tenant::DEFAULT_TENANT).await.unwrap(),
            b.next_id(crate::tenant::DEFAULT_TENANT).await.unwrap()
        );
    }

    #[tokio::test]
    async fn next_id_overflow_does_not_advance_the_counter() {
        let dir = tempfile::tempdir().unwrap();
        let s = LmdbStore::open_with_node_id(dir.path(), Some(7)).unwrap();
        {
            let mut wtxn = s.env.write_txn().unwrap();
            s.meta.put(&mut wtxn, "next_id", &(LOCAL_MAX - 1)).unwrap();
            wtxn.commit().unwrap();
        }
        let last = s.next_id(crate::tenant::DEFAULT_TENANT).await.unwrap();
        assert_eq!(last, (7u64 << LOCAL_BITS) | LOCAL_MAX);
        assert!(matches!(
            s.next_id(crate::tenant::DEFAULT_TENANT).await,
            Err(crate::store::StoreError::IdSpaceExhausted)
        ));
        let rtxn = s.env.read_txn().unwrap();
        assert_eq!(s.meta.get(&rtxn, "next_id").unwrap(), Some(LOCAL_MAX));
    }

    #[tokio::test]
    async fn wellknown_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let s = LmdbStore::open_with_node_id(dir.path(), None).unwrap();
        assert_eq!(
            s.get_wellknown(crate::tenant::DEFAULT_TENANT, "assetlinks.json")
                .await
                .unwrap(),
            None
        );
        let body = r#"{"relation":["delegate_permission/common.handle_all_urls"]}"#;
        s.put_wellknown(crate::tenant::DEFAULT_TENANT, "assetlinks.json", body)
            .await
            .unwrap();
        assert_eq!(
            s.get_wellknown(crate::tenant::DEFAULT_TENANT, "assetlinks.json")
                .await
                .unwrap(),
            Some(body.to_string())
        );
        s.delete_wellknown(crate::tenant::DEFAULT_TENANT, "assetlinks.json")
            .await
            .unwrap();
        assert_eq!(
            s.get_wellknown(crate::tenant::DEFAULT_TENANT, "assetlinks.json")
                .await
                .unwrap(),
            None
        );
        s.delete_wellknown(crate::tenant::DEFAULT_TENANT, "assetlinks.json")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn search_links_is_unsupported_on_lmdb() {
        let dir = tempfile::tempdir().unwrap();
        let store = LmdbStore::open_with_node_id(dir.path(), None).unwrap();
        let r = store
            .search_links(
                crate::tenant::DEFAULT_TENANT,
                "anything",
                None,
                10,
                None,
                None,
            )
            .await;
        assert!(matches!(r, Err(StoreError::Unsupported)));
    }

    #[tokio::test]
    async fn list_delete_links_and_aliases() {
        let dir = tempfile::tempdir().unwrap();
        let s = LmdbStore::open_with_node_id(dir.path(), None).unwrap();
        let rec = |u: &str| Record {
            url: u.into(),
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
        };
        for id in 1..=5u64 {
            s.put_link(
                crate::tenant::DEFAULT_TENANT,
                id,
                &rec(&format!("https://e{id}.com")),
            )
            .await
            .unwrap();
        }
        s.put_alias_and_link(
            crate::tenant::DEFAULT_TENANT,
            "promo",
            10,
            &rec("https://promo.com"),
        )
        .await
        .unwrap();

        let p1 = s
            .list_links(crate::tenant::DEFAULT_TENANT, None, 3, None, None)
            .await
            .unwrap();
        assert_eq!(
            p1.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        let p2 = s
            .list_links(crate::tenant::DEFAULT_TENANT, Some(3), 3, None, None)
            .await
            .unwrap();
        assert_eq!(
            p2.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![4, 5, 10]
        );

        let al = s.list_aliases(crate::tenant::DEFAULT_TENANT).await.unwrap();
        assert_eq!(al, vec![("promo".to_string(), 10u64)]);

        s.delete_link(crate::tenant::DEFAULT_TENANT, 2)
            .await
            .unwrap();
        assert!(s
            .get_link(crate::tenant::DEFAULT_TENANT, 2)
            .await
            .unwrap()
            .is_none());
        s.delete_alias(crate::tenant::DEFAULT_TENANT, "promo")
            .await
            .unwrap();
        assert_eq!(
            s.get_alias(crate::tenant::DEFAULT_TENANT, "promo")
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn webhook_crud_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = LmdbStore::open_with_node_id(dir.path(), None).unwrap();
        let id = store
            .next_webhook_id(crate::tenant::DEFAULT_TENANT)
            .await
            .unwrap();
        let sub = WebhookSubscription {
            id,
            url: "https://e.com".into(),
            events: vec![EventType::LinkCreated],
            secret: "whsec_a".into(),
            active: true,
            created: 1,
            kind: crate::webhooks::SubscriptionKind::Generic,
        };
        store
            .put_webhook(crate::tenant::DEFAULT_TENANT, &sub)
            .await
            .unwrap();
        assert_eq!(
            store
                .get_webhook(crate::tenant::DEFAULT_TENANT, id)
                .await
                .unwrap()
                .unwrap()
                .url,
            "https://e.com"
        );
        assert_eq!(
            store
                .list_webhooks(crate::tenant::DEFAULT_TENANT)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(store
            .delete_webhook(crate::tenant::DEFAULT_TENANT, id)
            .await
            .unwrap());
        assert!(store
            .get_webhook(crate::tenant::DEFAULT_TENANT, id)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn tags_round_trip_and_filter_and_distinct() {
        let dir = tempfile::tempdir().unwrap();
        let s = LmdbStore::open_with_node_id(dir.path(), None).unwrap();
        let rec = |u: &str, tags: &[&str]| Record {
            url: u.into(),
            expiry: None,
            created: 0,
            tags: tags.iter().map(|t| t.to_string()).collect(),
            max_visits: None,
            rules: Vec::new(),
            variants: Vec::new(),
            app_ios: None,
            app_android: None,
            folder: None,
            fallback_url: None,
            password_hash: None,
        };
        s.put_link(
            crate::tenant::DEFAULT_TENANT,
            1,
            &rec("https://a.com", &["rust", "web"]),
        )
        .await
        .unwrap();
        s.put_link(
            crate::tenant::DEFAULT_TENANT,
            2,
            &rec("https://b.com", &["web"]),
        )
        .await
        .unwrap();
        s.put_link(crate::tenant::DEFAULT_TENANT, 3, &rec("https://c.com", &[]))
            .await
            .unwrap();

        let got = s
            .get_link(crate::tenant::DEFAULT_TENANT, 1)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.tags, vec!["rust".to_string(), "web".to_string()]);

        let filtered = s
            .list_links(crate::tenant::DEFAULT_TENANT, None, 50, Some("rust"), None)
            .await
            .unwrap();
        assert_eq!(
            filtered.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![1]
        );

        let tags = s.list_tags(crate::tenant::DEFAULT_TENANT).await.unwrap();
        assert_eq!(tags, vec![("rust".to_string(), 1), ("web".to_string(), 2)]);
    }

    #[tokio::test]
    async fn folder_round_trip_filter_and_list() {
        let dir = tempfile::tempdir().unwrap();
        let s = LmdbStore::open_with_node_id(dir.path(), None).unwrap();
        let rec = |u: &str, folder: Option<&str>| Record {
            url: u.into(),
            expiry: None,
            created: 0,
            tags: Vec::new(),
            max_visits: None,
            rules: Vec::new(),
            variants: Vec::new(),
            app_ios: None,
            app_android: None,
            folder: folder.map(str::to_string),
            fallback_url: None,
            password_hash: None,
        };
        s.put_link(
            crate::tenant::DEFAULT_TENANT,
            1,
            &rec("https://a.com", Some("Marketing")),
        )
        .await
        .unwrap();
        s.put_link(
            crate::tenant::DEFAULT_TENANT,
            2,
            &rec("https://b.com", Some("Marketing")),
        )
        .await
        .unwrap();
        s.put_link(
            crate::tenant::DEFAULT_TENANT,
            3,
            &rec("https://c.com", Some("Docs")),
        )
        .await
        .unwrap();
        s.put_link(
            crate::tenant::DEFAULT_TENANT,
            4,
            &rec("https://d.com", None),
        )
        .await
        .unwrap();

        let got = s
            .get_link(crate::tenant::DEFAULT_TENANT, 1)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.folder.as_deref(), Some("Marketing"));

        let filtered = s
            .list_links(
                crate::tenant::DEFAULT_TENANT,
                None,
                50,
                None,
                Some("marketing"),
            )
            .await
            .unwrap();
        assert_eq!(
            filtered.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![1, 2]
        );

        let folders = s.list_folders(crate::tenant::DEFAULT_TENANT).await.unwrap();
        assert_eq!(
            folders,
            vec![("Docs".to_string(), 1u64), ("Marketing".to_string(), 2u64)]
        );
    }

    fn sample_token(id: u64, hash: String) -> ApiToken {
        ApiToken {
            id,
            name: "ci".into(),
            token_hash: hash,
            scopes: vec![Scope::LinksRead],
            rate_limit_per_min: Some(60),
            created: 1,
            tenant_id: crate::tenant::DEFAULT_TENANT,
        }
    }

    #[tokio::test]
    async fn api_token_crud_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = LmdbStore::open_with_node_id(dir.path(), None).unwrap();
        let id = store
            .next_api_token_id(crate::tenant::DEFAULT_TENANT)
            .await
            .unwrap();
        let hash = hash_token("qtok_abc123");
        let token = sample_token(id, hash.clone());
        store
            .put_api_token(crate::tenant::DEFAULT_TENANT, &token)
            .await
            .unwrap();

        assert_eq!(
            store.get_api_token_by_hash(&hash).await.unwrap(),
            Some(token)
        );
        assert_eq!(
            store
                .list_api_tokens(crate::tenant::DEFAULT_TENANT)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(store
            .delete_api_token(crate::tenant::DEFAULT_TENANT, id)
            .await
            .unwrap());
        assert_eq!(store.get_api_token_by_hash(&hash).await.unwrap(), None);
    }

    #[tokio::test]
    async fn get_api_token_by_hash_returns_none_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let store = LmdbStore::open_with_node_id(dir.path(), None).unwrap();
        assert_eq!(
            store.get_api_token_by_hash("no-such-hash").await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn delete_api_token_returns_false_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let store = LmdbStore::open_with_node_id(dir.path(), None).unwrap();
        assert!(!store
            .delete_api_token(crate::tenant::DEFAULT_TENANT, 999)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn next_api_token_id_increments() {
        let dir = tempfile::tempdir().unwrap();
        let store = LmdbStore::open_with_node_id(dir.path(), None).unwrap();
        let a = store
            .next_api_token_id(crate::tenant::DEFAULT_TENANT)
            .await
            .unwrap();
        let b = store
            .next_api_token_id(crate::tenant::DEFAULT_TENANT)
            .await
            .unwrap();
        assert_eq!(b, a + 1);
    }

    #[tokio::test]
    async fn pixel_round_trip_list_delete() {
        let dir = tempfile::tempdir().unwrap();
        let s = LmdbStore::open_with_node_id(dir.path(), None).unwrap();

        assert_eq!(
            s.next_pixel_id(crate::tenant::DEFAULT_TENANT)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            s.next_pixel_id(crate::tenant::DEFAULT_TENANT)
                .await
                .unwrap(),
            2
        );

        let config = PixelConfig {
            id: 1,
            provider: Provider::Ga4,
            credentials: PixelCredentials {
                measurement_id: Some("G-1".into()),
                api_secret: Some("s".into()),
                pixel_id: None,
                access_token: None,
            },
            active: true,
            created: 42,
        };
        s.put_pixel(crate::tenant::DEFAULT_TENANT, &config)
            .await
            .unwrap();

        let got = s
            .get_pixel(crate::tenant::DEFAULT_TENANT, 1)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.provider, Provider::Ga4);
        assert_eq!(got.credentials.measurement_id.as_deref(), Some("G-1"));
        assert!(got.active);
        assert_eq!(got.created, 42);

        assert!(s
            .get_pixel(crate::tenant::DEFAULT_TENANT, 999)
            .await
            .unwrap()
            .is_none());

        let list = s.list_pixels(crate::tenant::DEFAULT_TENANT).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, 1);

        assert!(s
            .delete_pixel(crate::tenant::DEFAULT_TENANT, 1)
            .await
            .unwrap());
        assert!(!s
            .delete_pixel(crate::tenant::DEFAULT_TENANT, 1)
            .await
            .unwrap());
        assert!(s
            .get_pixel(crate::tenant::DEFAULT_TENANT, 1)
            .await
            .unwrap()
            .is_none());
        assert!(s
            .list_pixels(crate::tenant::DEFAULT_TENANT)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn app_destinations_survive_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let s = LmdbStore::open_with_node_id(dir.path(), None).unwrap();
        let rec = Record {
            url: "https://example.com".into(),
            expiry: None,
            created: 0,
            tags: Vec::new(),
            max_visits: None,
            rules: Vec::new(),
            variants: Vec::new(),
            app_ios: Some("https://apps.apple.com/x".into()),
            app_android: None,
            folder: None,
            fallback_url: None,
            password_hash: None,
        };
        s.put_link(crate::tenant::DEFAULT_TENANT, 1, &rec)
            .await
            .unwrap();
        let got = s
            .get_link(crate::tenant::DEFAULT_TENANT, 1)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.app_ios.as_deref(), Some("https://apps.apple.com/x"));
        assert_eq!(got.app_android, None);
    }

    #[tokio::test]
    async fn session_round_trip_expiry_and_gc() {
        let dir = tempfile::tempdir().unwrap();
        let s = LmdbStore::open_with_node_id(dir.path(), None).unwrap();
        let sess = crate::auth::Session {
            token_hash: "abc".into(),
            subject: "sub-1".into(),
            display: "a@example.com".into(),
            scopes: vec![crate::auth::Scope::Full],
            created: 10,
            expires: 100,
            tenant_id: crate::tenant::DEFAULT_TENANT,
            user_id: 0,
        };
        s.put_session(crate::tenant::DEFAULT_TENANT, &sess)
            .await
            .unwrap();
        // Valid before expiry.
        assert_eq!(
            s.get_session_by_hash("abc", 50)
                .await
                .unwrap()
                .unwrap()
                .subject,
            "sub-1"
        );
        // Expired: not returned even though the row still exists.
        assert!(s.get_session_by_hash("abc", 100).await.unwrap().is_none());
        assert!(s
            .get_session_by_hash("missing", 50)
            .await
            .unwrap()
            .is_none());
        // Delete (logout).
        s.delete_session("abc").await.unwrap();
        assert!(s.get_session_by_hash("abc", 50).await.unwrap().is_none());
        // gc removes expired rows.
        s.put_session(
            crate::tenant::DEFAULT_TENANT,
            &crate::auth::Session {
                token_hash: "old".into(),
                expires: 5,
                ..sess.clone()
            },
        )
        .await
        .unwrap();
        s.put_session(
            crate::tenant::DEFAULT_TENANT,
            &crate::auth::Session {
                token_hash: "new".into(),
                expires: 999,
                ..sess.clone()
            },
        )
        .await
        .unwrap();
        s.gc_sessions(50).await.unwrap();
        assert!(s.get_session_by_hash("old", 4).await.unwrap().is_none());
        assert!(s.get_session_by_hash("new", 50).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn link_health_round_trip_and_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let s = LmdbStore::open_with_node_id(dir.path(), None).unwrap();
        assert!(s
            .list_link_health(crate::tenant::DEFAULT_TENANT)
            .await
            .unwrap()
            .is_empty());

        s.put_link_health(
            crate::tenant::DEFAULT_TENANT,
            1,
            &crate::store::LinkHealth {
                checked_at: 100,
                status: Some(200),
                healthy: true,
            },
        )
        .await
        .unwrap();
        s.put_link_health(
            crate::tenant::DEFAULT_TENANT,
            2,
            &crate::store::LinkHealth {
                checked_at: 100,
                status: Some(404),
                healthy: false,
            },
        )
        .await
        .unwrap();
        let all = s
            .list_link_health(crate::tenant::DEFAULT_TENANT)
            .await
            .unwrap();
        assert_eq!(all.len(), 2);

        // Overwrite id 1 (recovered -> broken).
        s.put_link_health(
            crate::tenant::DEFAULT_TENANT,
            1,
            &crate::store::LinkHealth {
                checked_at: 200,
                status: None,
                healthy: false,
            },
        )
        .await
        .unwrap();
        let map: std::collections::HashMap<u64, crate::store::LinkHealth> = s
            .list_link_health(crate::tenant::DEFAULT_TENANT)
            .await
            .unwrap()
            .into_iter()
            .collect();
        assert_eq!(map.len(), 2);
        assert_eq!(
            map[&1],
            crate::store::LinkHealth {
                checked_at: 200,
                status: None,
                healthy: false
            }
        );
        assert!(!map[&2].healthy);
    }

    #[test]
    fn record_without_app_fields_deserializes_to_none() {
        let blob = r#"{"url":"https://example.com","expiry":null,"created":7}"#;
        let rec: Record = serde_json::from_str(blob).unwrap();
        assert_eq!(rec.app_ios, None);
        assert_eq!(rec.app_android, None);
    }
}
