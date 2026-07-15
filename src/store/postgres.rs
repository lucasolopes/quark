use crate::analytics::{is_bot, Aggregates, AnalyticsSink, ClickEvent, Stats, EVENTS_MAX};
use crate::auth::ApiToken;
use crate::pixel::{PixelConfig, PixelCredentials, Provider};
use crate::store::{LinkHealth, OutboxDelivery, OutboxRow, Record, Store, StoreError, Variant};
use crate::webhooks::{SubscriptionKind, WebhookSubscription};
use sqlx::postgres::{PgPoolOptions, PgRow};
use sqlx::{PgPool, Row};

/// Key of the pg_advisory_lock that serializes idempotent schema creation across instances.
const QUARK_SCHEMA_LOCK_ID: i64 = 727271;

/// Visibility lease (seconds) applied by `claim_due_deliveries`: a claimed row
/// has its `next_attempt_at` pushed this far out so a concurrent relay skips
/// it, while a relay that crashes mid-delivery has the row re-claimed once the
/// lease expires (at-least-once). Comfortably longer than one delivery attempt.
const CLAIM_LEASE_SECS: u64 = 60;

/// Escapes `LIKE`/`ILIKE` wildcards (default escape char = `\`) so that the
/// user's term is treated literally. Order matters: escape `\` first.
fn like_escape(q: &str) -> String {
    q.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Maps a `links` row (id, url, expiry, created, tags, max_visits, rules,
/// variants) into `(id, Record)`.
/// Shared by `list_links` and `search_links`, which select the same columns.
fn row_to_link(r: &PgRow) -> Result<(u64, Record), StoreError> {
    let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
    let url: String = r.try_get("url").map_err(StoreError::backend)?;
    let expiry: Option<i64> = r.try_get("expiry").map_err(StoreError::backend)?;
    let created: i64 = r.try_get("created").map_err(StoreError::backend)?;
    let tags: serde_json::Value = r.try_get("tags").map_err(StoreError::backend)?;
    let tags: Vec<String> = serde_json::from_value(tags)?;
    let max_visits: Option<i64> = r.try_get("max_visits").map_err(StoreError::backend)?;
    let rules: serde_json::Value = r.try_get("rules").map_err(StoreError::backend)?;
    let rules: Vec<crate::store::Rule> = serde_json::from_value(rules)?;
    let variants: serde_json::Value = r.try_get("variants").map_err(StoreError::backend)?;
    let variants: Vec<Variant> = serde_json::from_value(variants)?;
    let app_ios: Option<String> = r.try_get("app_ios").map_err(StoreError::backend)?;
    let app_android: Option<String> = r.try_get("app_android").map_err(StoreError::backend)?;
    let folder: Option<String> = r.try_get("folder").map_err(StoreError::backend)?;
    let fallback_url: Option<String> =
        r.try_get("fallback_url").map_err(StoreError::backend)?;
    let password_hash: Option<String> =
        r.try_get("password_hash").map_err(StoreError::backend)?;
    Ok((
        id as u64,
        Record {
            url,
            expiry: expiry.map(|v| v as u64),
            created: created as u64,
            tags,
            max_visits: max_visits.map(|v| v as u32),
            rules,
            variants,
            app_ios,
            app_android,
            folder,
            fallback_url,
            password_hash,
        },
    ))
}

/// Maps a `webhooks` row into a `WebhookSubscription`.
fn row_to_webhook(r: &PgRow) -> Result<WebhookSubscription, StoreError> {
    let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
    let url: String = r.try_get("url").map_err(StoreError::backend)?;
    let events: serde_json::Value = r.try_get("events").map_err(StoreError::backend)?;
    let secret: String = r.try_get("secret").map_err(StoreError::backend)?;
    let active: bool = r.try_get("active").map_err(StoreError::backend)?;
    let created: i64 = r.try_get("created").map_err(StoreError::backend)?;
    let kind: String = r.try_get("kind").map_err(StoreError::backend)?;
    Ok(WebhookSubscription {
        id: id as u64,
        url,
        events: serde_json::from_value(events)?,
        secret,
        active,
        created: created as u64,
        kind: SubscriptionKind::from_str_or_generic(&kind),
    })
}

/// Maps a `webhook_deliveries` row (the columns `claim_due_deliveries`
/// returns) into an `OutboxDelivery`.
fn row_to_delivery(r: &PgRow) -> Result<OutboxDelivery, StoreError> {
    let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
    let delivery_key: String = r.try_get("delivery_key").map_err(StoreError::backend)?;
    let subscription_id: i64 = r.try_get("subscription_id").map_err(StoreError::backend)?;
    let event_type: String = r.try_get("event_type").map_err(StoreError::backend)?;
    let payload: String = r.try_get("payload").map_err(StoreError::backend)?;
    let attempts: i32 = r.try_get("attempts").map_err(StoreError::backend)?;
    Ok(OutboxDelivery {
        id,
        delivery_key,
        subscription_id: subscription_id as u64,
        event_type,
        payload,
        attempts: attempts as u32,
    })
}

/// Maps an `api_tokens` row into an `ApiToken`.
fn row_to_api_token(r: &PgRow) -> Result<ApiToken, StoreError> {
    let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
    let name: String = r.try_get("name").map_err(StoreError::backend)?;
    let token_hash: String = r.try_get("token_hash").map_err(StoreError::backend)?;
    let scopes: serde_json::Value = r.try_get("scopes").map_err(StoreError::backend)?;
    let rate_limit_per_min: Option<i64> = r
        .try_get("rate_limit_per_min")
        .map_err(StoreError::backend)?;
    let created: i64 = r.try_get("created").map_err(StoreError::backend)?;
    Ok(ApiToken {
        id: id as u64,
        name,
        token_hash,
        scopes: serde_json::from_value(scopes)?,
        rate_limit_per_min: rate_limit_per_min.map(|v| v as u32),
        created: created as u64,
    })
}

/// Maps a `Provider` to the string stored in the `pixels.provider` column.
fn provider_to_str(p: Provider) -> &'static str {
    match p {
        Provider::Ga4 => "ga4",
        Provider::MetaCapi => "meta_capi",
    }
}

/// Inverse of `provider_to_str`. Errors on an unrecognized value (defensive
/// against manual DB edits or a future provider not yet handled here).
fn provider_from_str(s: &str) -> Result<Provider, StoreError> {
    match s {
        "ga4" => Ok(Provider::Ga4),
        "meta_capi" => Ok(Provider::MetaCapi),
        other => Err(StoreError::Backend(format!(
            "unknown pixel provider: {other}"
        ))),
    }
}

/// Maps a `pixels` row into a `PixelConfig`.
fn row_to_pixel(r: &PgRow) -> Result<PixelConfig, StoreError> {
    let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
    let provider: String = r.try_get("provider").map_err(StoreError::backend)?;
    let credentials: serde_json::Value = r.try_get("credentials").map_err(StoreError::backend)?;
    let active: bool = r.try_get("active").map_err(StoreError::backend)?;
    let created: i64 = r.try_get("created").map_err(StoreError::backend)?;
    Ok(PixelConfig {
        id: id as u64,
        provider: provider_from_str(&provider)?,
        credentials: serde_json::from_value::<PixelCredentials>(credentials)?,
        active,
        created: created as u64,
    })
}

/// Upserts a link row inside an open transaction (same SQL as `put_link`).
/// Shared by the `_tx` mutation methods so the link write and the outbox
/// enqueue commit atomically.
async fn upsert_link_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: u64,
    rec: &Record,
) -> Result<(), StoreError> {
    let tags = serde_json::to_value(&rec.tags)?;
    let rules = serde_json::to_value(&rec.rules)?;
    let variants = serde_json::to_value(&rec.variants)?;
    sqlx::query(
        "INSERT INTO links (id, url, expiry, created, tags, max_visits, rules, variants, app_ios, app_android, folder, fallback_url, password_hash) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) \
         ON CONFLICT (id) DO UPDATE SET url=$2, expiry=$3, created=$4, tags=$5, max_visits=$6, rules=$7, variants=$8, app_ios=$9, app_android=$10, folder=$11, fallback_url=$12, password_hash=$13",
    )
    .bind(id as i64)
    .bind(&rec.url)
    .bind(rec.expiry.map(|v| v as i64))
    .bind(rec.created as i64)
    .bind(&tags)
    .bind(rec.max_visits.map(|v| v as i64))
    .bind(&rules)
    .bind(&variants)
    .bind(&rec.app_ios)
    .bind(&rec.app_android)
    .bind(&rec.folder)
    .bind(&rec.fallback_url)
    .bind(&rec.password_hash)
    .execute(&mut **tx)
    .await
    .map_err(StoreError::backend)?;
    Ok(())
}

/// Enqueues the webhook-outbox `rows` inside an open transaction, with
/// `ON CONFLICT (delivery_key) DO NOTHING` (same idempotent insert as
/// `enqueue_deliveries`, but sharing the mutation's transaction). An empty
/// slice is a no-op (the LMDB case never reaches here).
async fn enqueue_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rows: &[OutboxRow],
) -> Result<(), StoreError> {
    for row in rows {
        sqlx::query(
            "INSERT INTO webhook_deliveries (delivery_key, subscription_id, event_type, payload, created, next_attempt_at) \
             VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT (delivery_key) DO NOTHING",
        )
        .bind(&row.delivery_key)
        .bind(row.subscription_id as i64)
        .bind(&row.event_type)
        .bind(&row.payload)
        .bind(row.created as i64)
        .bind(row.next_attempt_at as i64)
        .execute(&mut **tx)
        .await
        .map_err(StoreError::backend)?;
    }
    Ok(())
}

pub struct PostgresStore {
    pool: PgPool,
}

impl PostgresStore {
    pub async fn open(url: &str) -> Result<PostgresStore, StoreError> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(url)
            .await
            .map_err(StoreError::backend)?;
        let s = PostgresStore { pool };
        s.init_schema().await?;
        Ok(s)
    }

    /// Creates the schema idempotently. `CREATE TABLE/SEQUENCE IF NOT EXISTS`
    /// can still collide under concurrency (several connections check "doesn't exist"
    /// and try to create at the same time, hitting the Postgres catalog's unique
    /// constraints) — so we serialize with a session advisory lock on a
    /// single connection before running the DDL.
    async fn init_schema(&self) -> Result<(), StoreError> {
        let mut conn = self.pool.acquire().await.map_err(StoreError::backend)?;
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(QUARK_SCHEMA_LOCK_ID)
            .execute(&mut *conn)
            .await
            .map_err(StoreError::backend)?;

        let result = async {
            for ddl in [
                "CREATE SEQUENCE IF NOT EXISTS quark_id_seq",
                "CREATE SEQUENCE IF NOT EXISTS quark_webhook_id_seq",
                "CREATE SEQUENCE IF NOT EXISTS quark_api_token_id_seq",
                "CREATE TABLE IF NOT EXISTS links (id BIGINT PRIMARY KEY, url TEXT NOT NULL, expiry BIGINT, created BIGINT NOT NULL, tags JSONB NOT NULL DEFAULT '[]')",
                "ALTER TABLE links ADD COLUMN IF NOT EXISTS tags JSONB NOT NULL DEFAULT '[]'",
                "ALTER TABLE links ADD COLUMN IF NOT EXISTS rules JSONB NOT NULL DEFAULT '[]'",
                "ALTER TABLE links ADD COLUMN IF NOT EXISTS app_ios TEXT",
                "ALTER TABLE links ADD COLUMN IF NOT EXISTS app_android TEXT",
                "ALTER TABLE links ADD COLUMN IF NOT EXISTS folder TEXT",
                "ALTER TABLE links ADD COLUMN IF NOT EXISTS fallback_url TEXT",
                "ALTER TABLE links ADD COLUMN IF NOT EXISTS password_hash TEXT",
                "CREATE TABLE IF NOT EXISTS aliases (alias TEXT PRIMARY KEY, id BIGINT NOT NULL)",
                "CREATE TABLE IF NOT EXISTS link_health (id BIGINT PRIMARY KEY, checked_at BIGINT NOT NULL, status INT, healthy BOOLEAN NOT NULL)",
                "CREATE TABLE IF NOT EXISTS health_lease (id INT PRIMARY KEY, holder TEXT NOT NULL, expires_at BIGINT NOT NULL)",
                "CREATE TABLE IF NOT EXISTS stats (id BIGINT PRIMARY KEY, agg JSONB NOT NULL)",
                "CREATE TABLE IF NOT EXISTS events (id BIGINT PRIMARY KEY, recent JSONB NOT NULL)",
                "CREATE TABLE IF NOT EXISTS webhooks (id BIGINT PRIMARY KEY, url TEXT NOT NULL, events JSONB NOT NULL, secret TEXT NOT NULL, active BOOLEAN NOT NULL, created BIGINT NOT NULL, kind TEXT NOT NULL DEFAULT 'generic')",
                // `kind` (#6, native chat channels) is added after the fact for
                // deployments whose `webhooks` table predates it; pre-existing
                // rows have no kind opinion, so they default to `generic`
                // (same fallback `SubscriptionKind::from_str_or_generic` and
                // the LMDB/serde `#[serde(default)]` path use).
                "ALTER TABLE webhooks ADD COLUMN IF NOT EXISTS kind TEXT NOT NULL DEFAULT 'generic'",
                "CREATE TABLE IF NOT EXISTS api_tokens (id BIGINT PRIMARY KEY, name TEXT NOT NULL, token_hash TEXT NOT NULL, scopes JSONB NOT NULL, rate_limit_per_min BIGINT, created BIGINT NOT NULL)",
                "CREATE INDEX IF NOT EXISTS api_tokens_token_hash_idx ON api_tokens (token_hash)",
                // Idempotent migrations for pre-existing `links` tables (max-visits feature).
                "ALTER TABLE links ADD COLUMN IF NOT EXISTS max_visits BIGINT",
                "ALTER TABLE links ADD COLUMN IF NOT EXISTS visits BIGINT NOT NULL DEFAULT 0",
                "CREATE SEQUENCE IF NOT EXISTS quark_pixel_id_seq",
                "CREATE TABLE IF NOT EXISTS pixels (id BIGINT PRIMARY KEY, provider TEXT NOT NULL, credentials JSONB NOT NULL, active BOOLEAN NOT NULL, created BIGINT NOT NULL)",
                // Idempotent migration for a `links` table created before variants
                // existed (#17).
                "ALTER TABLE links ADD COLUMN IF NOT EXISTS variants JSONB NOT NULL DEFAULT '[]'",
                "CREATE TABLE IF NOT EXISTS wellknown_documents (name TEXT PRIMARY KEY, body TEXT NOT NULL)",
                // Atomic analytics (scale-audit #4): counters incremented with
                // `ON CONFLICT DO UPDATE SET count = count + EXCLUDED.count`
                // instead of the old whole-blob read-modify-write under an
                // advisory lock. `stats`/`events` above are kept for
                // idempotency but no longer read or written.
                "CREATE TABLE IF NOT EXISTS click_counters (id BIGINT NOT NULL, dimension TEXT NOT NULL, bucket TEXT NOT NULL, count BIGINT NOT NULL, PRIMARY KEY (id, dimension, bucket))",
                "CREATE TABLE IF NOT EXISTS stats_meta (id BIGINT PRIMARY KEY, first_ts BIGINT NOT NULL, last_ts BIGINT NOT NULL)",
                "CREATE TABLE IF NOT EXISTS click_events (seq BIGSERIAL PRIMARY KEY, id BIGINT NOT NULL, ts BIGINT NOT NULL, referer TEXT, country TEXT, user_agent TEXT, city TEXT, variant INT, event_id TEXT NOT NULL DEFAULT '')",
                "CREATE INDEX IF NOT EXISTS click_events_id_seq_idx ON click_events (id, seq DESC)",
                // Durable webhook outbox (scale-audit #3): one row per (event,
                // subscription) delivery attempt-set. `delivery_key` UNIQUE
                // gives insert-time idempotency; the relay poll filters on
                // (dead, delivered_at, next_attempt_at), hence the index.
                "CREATE TABLE IF NOT EXISTS webhook_deliveries (id BIGSERIAL PRIMARY KEY, delivery_key TEXT UNIQUE NOT NULL, subscription_id BIGINT NOT NULL, event_type TEXT NOT NULL, payload TEXT NOT NULL, created BIGINT NOT NULL, attempts INT NOT NULL DEFAULT 0, next_attempt_at BIGINT NOT NULL, delivered_at BIGINT, dead BOOLEAN NOT NULL DEFAULT FALSE)",
                "CREATE INDEX IF NOT EXISTS webhook_deliveries_poll_idx ON webhook_deliveries (dead, delivered_at, next_attempt_at)",
            ] {
                sqlx::query(ddl)
                    .execute(&mut *conn)
                    .await
                    .map_err(StoreError::backend)?;
            }
            Ok(())
        }
        .await;

        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(QUARK_SCHEMA_LOCK_ID)
            .execute(&mut *conn)
            .await
            .map_err(StoreError::backend)?;

        result
    }

    /// Used in tests: resets all state.
    pub async fn reset_for_tests(&self) -> Result<(), StoreError> {
        for q in [
            "TRUNCATE links, aliases, link_health, health_lease, stats, events, webhooks, api_tokens, pixels, wellknown_documents, click_counters, stats_meta, click_events, webhook_deliveries RESTART IDENTITY",
            "ALTER SEQUENCE quark_id_seq RESTART WITH 1",
            "ALTER SEQUENCE quark_webhook_id_seq RESTART WITH 1",
            "ALTER SEQUENCE quark_api_token_id_seq RESTART WITH 1",
            "ALTER SEQUENCE quark_pixel_id_seq RESTART WITH 1",
        ] {
            sqlx::query(q)
                .execute(&self.pool)
                .await
                .map_err(StoreError::backend)?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl Store for PostgresStore {
    async fn next_id(&self) -> Result<u64, StoreError> {
        let row = sqlx::query("SELECT nextval('quark_id_seq') AS id")
            .fetch_one(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        let id: i64 = row.try_get("id").map_err(StoreError::backend)?;
        Ok(id as u64)
    }

    async fn get_link(&self, id: u64) -> Result<Option<Record>, StoreError> {
        let row = sqlx::query(
            "SELECT id, url, expiry, created, tags, max_visits, rules, variants, app_ios, app_android, folder, fallback_url, password_hash FROM links WHERE id = $1",
        )
        .bind(id as i64)
        .fetch_optional(&self.pool)
        .await
        .map_err(StoreError::backend)?;
        match row {
            Some(r) => Ok(Some(row_to_link(&r)?.1)),
            None => Ok(None),
        }
    }

    async fn put_link(&self, id: u64, rec: &Record) -> Result<(), StoreError> {
        let tags = serde_json::to_value(&rec.tags)?;
        let rules = serde_json::to_value(&rec.rules)?;
        let variants = serde_json::to_value(&rec.variants)?;
        sqlx::query(
            "INSERT INTO links (id, url, expiry, created, tags, max_visits, rules, variants, app_ios, app_android, folder, fallback_url, password_hash) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) \
             ON CONFLICT (id) DO UPDATE SET url=$2, expiry=$3, created=$4, tags=$5, max_visits=$6, rules=$7, variants=$8, app_ios=$9, app_android=$10, folder=$11, fallback_url=$12, password_hash=$13",
        )
        .bind(id as i64)
        .bind(&rec.url)
        .bind(rec.expiry.map(|v| v as i64))
        .bind(rec.created as i64)
        .bind(&tags)
        .bind(rec.max_visits.map(|v| v as i64))
        .bind(&rules)
        .bind(&variants)
        .bind(&rec.app_ios)
        .bind(&rec.app_android)
        .bind(&rec.folder)
        .bind(&rec.fallback_url)
        .bind(&rec.password_hash)
        .execute(&self.pool)
        .await
        .map_err(StoreError::backend)?;
        Ok(())
    }

    async fn get_alias(&self, alias: &str) -> Result<Option<u64>, StoreError> {
        let row = sqlx::query("SELECT id FROM aliases WHERE alias = $1")
            .bind(alias)
            .fetch_optional(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        match row {
            Some(r) => {
                let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
                Ok(Some(id as u64))
            }
            None => Ok(None),
        }
    }

    async fn put_alias_and_link(
        &self,
        alias: &str,
        id: u64,
        rec: &Record,
    ) -> Result<bool, StoreError> {
        let mut tx = self.pool.begin().await.map_err(StoreError::backend)?;
        let res = sqlx::query(
            "INSERT INTO aliases (alias, id) VALUES ($1,$2) ON CONFLICT (alias) DO NOTHING",
        )
        .bind(alias)
        .bind(id as i64)
        .execute(&mut *tx)
        .await
        .map_err(StoreError::backend)?;
        if res.rows_affected() == 0 {
            return Ok(false);
        }
        let tags = serde_json::to_value(&rec.tags)?;
        let rules = serde_json::to_value(&rec.rules)?;
        let variants = serde_json::to_value(&rec.variants)?;
        sqlx::query(
            "INSERT INTO links (id, url, expiry, created, tags, max_visits, rules, variants, app_ios, app_android, folder, fallback_url, password_hash) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) \
             ON CONFLICT (id) DO UPDATE SET url=$2, expiry=$3, created=$4, tags=$5, max_visits=$6, rules=$7, variants=$8, app_ios=$9, app_android=$10, folder=$11, fallback_url=$12, password_hash=$13",
        )
        .bind(id as i64)
        .bind(&rec.url)
        .bind(rec.expiry.map(|v| v as i64))
        .bind(rec.created as i64)
        .bind(&tags)
        .bind(rec.max_visits.map(|v| v as i64))
        .bind(&rules)
        .bind(&variants)
        .bind(&rec.app_ios)
        .bind(&rec.app_android)
        .bind(&rec.folder)
        .bind(&rec.fallback_url)
        .bind(&rec.password_hash)
        .execute(&mut *tx)
        .await
        .map_err(StoreError::backend)?;
        tx.commit().await.map_err(StoreError::backend)?;
        Ok(true)
    }

    async fn put_link_tx(
        &self,
        id: u64,
        rec: &Record,
        deliveries: &[OutboxRow],
    ) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await.map_err(StoreError::backend)?;
        upsert_link_in_tx(&mut tx, id, rec).await?;
        enqueue_in_tx(&mut tx, deliveries).await?;
        tx.commit().await.map_err(StoreError::backend)?;
        Ok(())
    }

    async fn put_alias_and_link_tx(
        &self,
        alias: &str,
        id: u64,
        rec: &Record,
        deliveries: &[OutboxRow],
    ) -> Result<bool, StoreError> {
        let mut tx = self.pool.begin().await.map_err(StoreError::backend)?;
        let res = sqlx::query(
            "INSERT INTO aliases (alias, id) VALUES ($1,$2) ON CONFLICT (alias) DO NOTHING",
        )
        .bind(alias)
        .bind(id as i64)
        .execute(&mut *tx)
        .await
        .map_err(StoreError::backend)?;
        if res.rows_affected() == 0 {
            return Ok(false);
        }
        upsert_link_in_tx(&mut tx, id, rec).await?;
        enqueue_in_tx(&mut tx, deliveries).await?;
        tx.commit().await.map_err(StoreError::backend)?;
        Ok(true)
    }

    async fn delete_link_tx(&self, id: u64, deliveries: &[OutboxRow]) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await.map_err(StoreError::backend)?;
        sqlx::query("DELETE FROM links WHERE id = $1")
            .bind(id as i64)
            .execute(&mut *tx)
            .await
            .map_err(StoreError::backend)?;
        sqlx::query("DELETE FROM link_health WHERE id = $1")
            .bind(id as i64)
            .execute(&mut *tx)
            .await
            .map_err(StoreError::backend)?;
        enqueue_in_tx(&mut tx, deliveries).await?;
        tx.commit().await.map_err(StoreError::backend)?;
        Ok(())
    }

    async fn list_links(
        &self,
        after: Option<u64>,
        limit: usize,
        tag: Option<&str>,
        folder: Option<&str>,
    ) -> Result<Vec<(u64, Record)>, StoreError> {
        let tag_json = tag.map(|t| serde_json::json!([t]));
        let rows = sqlx::query(
            "SELECT id, url, expiry, created, tags, max_visits, rules, variants, app_ios, app_android, folder, fallback_url, password_hash FROM links \
             WHERE ($1::bigint IS NULL OR id > $1) \
               AND ($2::jsonb IS NULL OR tags @> $2) \
               AND ($4::text IS NULL OR lower(folder) = lower($4)) \
             ORDER BY id LIMIT $3",
        )
        .bind(after.map(|a| a as i64))
        .bind(&tag_json)
        .bind(limit as i64)
        .bind(folder)
        .fetch_all(&self.pool)
        .await
        .map_err(StoreError::backend)?;
        rows.iter().map(row_to_link).collect()
    }

    async fn search_links(
        &self,
        q: &str,
        after: Option<u64>,
        limit: usize,
        tag: Option<&str>,
        folder: Option<&str>,
    ) -> Result<Vec<(u64, Record)>, StoreError> {
        let pattern = format!("%{}%", like_escape(q));
        let tag_json = tag.map(|t| serde_json::json!([t]));
        let rows = sqlx::query(
            "SELECT DISTINCT l.id, l.url, l.expiry, l.created, l.tags, l.max_visits, l.rules, l.variants, l.app_ios, l.app_android, l.folder, l.fallback_url, l.password_hash \
             FROM links l LEFT JOIN aliases a ON a.id = l.id \
             WHERE ($1::bigint IS NULL OR l.id > $1) \
               AND (l.url ILIKE $2 OR a.alias ILIKE $2) \
               AND ($3::jsonb IS NULL OR l.tags @> $3) \
               AND ($5::text IS NULL OR lower(l.folder) = lower($5)) \
             ORDER BY l.id LIMIT $4",
        )
        .bind(after.map(|a| a as i64))
        .bind(&pattern)
        .bind(&tag_json)
        .bind(limit as i64)
        .bind(folder)
        .fetch_all(&self.pool)
        .await
        .map_err(StoreError::backend)?;
        rows.iter().map(row_to_link).collect()
    }

    async fn list_aliases(&self) -> Result<Vec<(String, u64)>, StoreError> {
        let rows = sqlx::query("SELECT alias, id FROM aliases")
            .fetch_all(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        let mut out = Vec::new();
        for r in rows {
            let alias: String = r.try_get("alias").map_err(StoreError::backend)?;
            let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
            out.push((alias, id as u64));
        }
        Ok(out)
    }

    async fn delete_link(&self, id: u64) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM links WHERE id = $1")
            .bind(id as i64)
            .execute(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        sqlx::query("DELETE FROM link_health WHERE id = $1")
            .bind(id as i64)
            .execute(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        Ok(())
    }

    async fn delete_alias(&self, alias: &str) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM aliases WHERE alias = $1")
            .bind(alias)
            .execute(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        Ok(())
    }

    async fn list_webhooks(&self) -> Result<Vec<WebhookSubscription>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, url, events, secret, active, created, kind FROM webhooks ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(StoreError::backend)?;
        rows.iter().map(row_to_webhook).collect()
    }

    async fn get_webhook(&self, id: u64) -> Result<Option<WebhookSubscription>, StoreError> {
        let row = sqlx::query(
            "SELECT id, url, events, secret, active, created, kind FROM webhooks WHERE id = $1",
        )
        .bind(id as i64)
        .fetch_optional(&self.pool)
        .await
        .map_err(StoreError::backend)?;
        match row {
            Some(r) => Ok(Some(row_to_webhook(&r)?)),
            None => Ok(None),
        }
    }

    async fn put_webhook(&self, sub: &WebhookSubscription) -> Result<(), StoreError> {
        let events = serde_json::to_value(&sub.events)?;
        sqlx::query(
            "INSERT INTO webhooks (id, url, events, secret, active, created, kind) VALUES ($1,$2,$3,$4,$5,$6,$7) \
             ON CONFLICT (id) DO UPDATE SET url=$2, events=$3, secret=$4, active=$5, created=$6, kind=$7",
        )
        .bind(sub.id as i64)
        .bind(&sub.url)
        .bind(&events)
        .bind(&sub.secret)
        .bind(sub.active)
        .bind(sub.created as i64)
        .bind(sub.kind.as_str())
        .execute(&self.pool)
        .await
        .map_err(StoreError::backend)?;
        Ok(())
    }

    async fn delete_webhook(&self, id: u64) -> Result<bool, StoreError> {
        let res = sqlx::query("DELETE FROM webhooks WHERE id = $1")
            .bind(id as i64)
            .execute(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        Ok(res.rows_affected() > 0)
    }

    async fn next_webhook_id(&self) -> Result<u64, StoreError> {
        let row = sqlx::query("SELECT nextval('quark_webhook_id_seq') AS id")
            .fetch_one(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        let id: i64 = row.try_get("id").map_err(StoreError::backend)?;
        Ok(id as u64)
    }

    async fn list_tags(&self) -> Result<Vec<(String, u64)>, StoreError> {
        // Dedupe tags within a link (SELECT DISTINCT per id) before counting,
        // so a link carrying the same tag twice still counts once.
        let rows = sqlx::query(
            "SELECT tag, count(*) AS n FROM ( \
               SELECT DISTINCT id, jsonb_array_elements_text(tags) AS tag FROM links \
             ) t GROUP BY tag ORDER BY tag",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(StoreError::backend)?;
        rows.iter()
            .map(|r| {
                let name: String = r.try_get("tag").map_err(StoreError::backend)?;
                let n: i64 = r.try_get("n").map_err(StoreError::backend)?;
                Ok((name, n as u64))
            })
            .collect()
    }

    async fn list_folders(&self) -> Result<Vec<(String, u64)>, StoreError> {
        let rows = sqlx::query(
            "SELECT folder, count(*) AS n FROM links WHERE folder IS NOT NULL GROUP BY folder ORDER BY folder",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(StoreError::backend)?;
        rows.iter()
            .map(|r| {
                let name: String = r.try_get("folder").map_err(StoreError::backend)?;
                let n: i64 = r.try_get("n").map_err(StoreError::backend)?;
                Ok((name, n as u64))
            })
            .collect()
    }

    async fn list_api_tokens(&self) -> Result<Vec<ApiToken>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, name, token_hash, scopes, rate_limit_per_min, created \
             FROM api_tokens ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(StoreError::backend)?;
        rows.iter().map(row_to_api_token).collect()
    }

    /// Hot path of auth: indexed lookup by `token_hash`
    /// (`api_tokens_token_hash_idx`).
    async fn get_api_token_by_hash(&self, hash: &str) -> Result<Option<ApiToken>, StoreError> {
        let row = sqlx::query(
            "SELECT id, name, token_hash, scopes, rate_limit_per_min, created \
             FROM api_tokens WHERE token_hash = $1",
        )
        .bind(hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(StoreError::backend)?;
        match row {
            Some(r) => Ok(Some(row_to_api_token(&r)?)),
            None => Ok(None),
        }
    }

    async fn put_api_token(&self, token: &ApiToken) -> Result<(), StoreError> {
        let scopes = serde_json::to_value(&token.scopes)?;
        sqlx::query(
            "INSERT INTO api_tokens (id, name, token_hash, scopes, rate_limit_per_min, created) \
             VALUES ($1,$2,$3,$4,$5,$6) \
             ON CONFLICT (id) DO UPDATE SET \
             name=$2, token_hash=$3, scopes=$4, rate_limit_per_min=$5, created=$6",
        )
        .bind(token.id as i64)
        .bind(&token.name)
        .bind(&token.token_hash)
        .bind(&scopes)
        .bind(token.rate_limit_per_min.map(|v| v as i64))
        .bind(token.created as i64)
        .execute(&self.pool)
        .await
        .map_err(StoreError::backend)?;
        Ok(())
    }

    async fn delete_api_token(&self, id: u64) -> Result<bool, StoreError> {
        let res = sqlx::query("DELETE FROM api_tokens WHERE id = $1")
            .bind(id as i64)
            .execute(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        Ok(res.rows_affected() > 0)
    }

    async fn next_api_token_id(&self) -> Result<u64, StoreError> {
        let row = sqlx::query("SELECT nextval('quark_api_token_id_seq') AS id")
            .fetch_one(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        let id: i64 = row.try_get("id").map_err(StoreError::backend)?;
        Ok(id as u64)
    }

    async fn bump_visits(&self, id: u64) -> Result<u64, StoreError> {
        let row =
            sqlx::query("UPDATE links SET visits = visits + 1 WHERE id = $1 RETURNING visits")
                .bind(id as i64)
                .fetch_one(&self.pool)
                .await
                .map_err(StoreError::backend)?;
        let visits: i64 = row.try_get("visits").map_err(StoreError::backend)?;
        Ok(visits as u64)
    }

    async fn visits(&self, id: u64) -> Result<u64, StoreError> {
        let row = sqlx::query("SELECT visits FROM links WHERE id = $1")
            .bind(id as i64)
            .fetch_optional(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        match row {
            Some(r) => {
                let visits: i64 = r.try_get("visits").map_err(StoreError::backend)?;
                Ok(visits as u64)
            }
            None => Ok(0),
        }
    }

    async fn put_link_health(&self, id: u64, health: &LinkHealth) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO link_health (id, checked_at, status, healthy) VALUES ($1,$2,$3,$4) \
             ON CONFLICT (id) DO UPDATE SET checked_at=$2, status=$3, healthy=$4",
        )
        .bind(id as i64)
        .bind(health.checked_at as i64)
        .bind(health.status.map(|s| s as i32))
        .bind(health.healthy)
        .execute(&self.pool)
        .await
        .map_err(StoreError::backend)?;
        Ok(())
    }

    async fn list_link_health(&self) -> Result<Vec<(u64, LinkHealth)>, StoreError> {
        let rows = sqlx::query("SELECT id, checked_at, status, healthy FROM link_health")
            .fetch_all(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
            let checked_at: i64 = r.try_get("checked_at").map_err(StoreError::backend)?;
            let status: Option<i32> = r.try_get("status").map_err(StoreError::backend)?;
            let healthy: bool = r.try_get("healthy").map_err(StoreError::backend)?;
            out.push((
                id as u64,
                LinkHealth {
                    checked_at: checked_at as u64,
                    status: status.map(|s| s as u16),
                    healthy,
                },
            ));
        }
        Ok(out)
    }

    async fn list_broken_link_ids(&self) -> Result<Vec<u64>, StoreError> {
        let rows = sqlx::query("SELECT id FROM link_health WHERE healthy = false ORDER BY id")
            .fetch_all(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
            out.push(id as u64);
        }
        Ok(out)
    }

    async fn try_acquire_health_lease(
        &self,
        holder: &str,
        ttl_secs: u64,
    ) -> Result<bool, StoreError> {
        let now = crate::now() as i64;
        let exp = now + ttl_secs as i64;
        // Claim if the lease is free/expired, or renew if we already hold it.
        let row = sqlx::query(
            "INSERT INTO health_lease (id, holder, expires_at) VALUES (1, $1, $2) \
             ON CONFLICT (id) DO UPDATE SET holder = $1, expires_at = $2 \
             WHERE health_lease.expires_at < $3 OR health_lease.holder = $1 \
             RETURNING holder",
        )
        .bind(holder)
        .bind(exp)
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .map_err(StoreError::backend)?;
        Ok(row.is_some())
    }

    async fn link_health_for(&self, ids: &[u64]) -> Result<Vec<(u64, LinkHealth)>, StoreError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let id_list: Vec<i64> = ids.iter().map(|&i| i as i64).collect();
        let rows = sqlx::query(
            "SELECT id, checked_at, status, healthy FROM link_health WHERE id = ANY($1)",
        )
        .bind(&id_list)
        .fetch_all(&self.pool)
        .await
        .map_err(StoreError::backend)?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
            let checked_at: i64 = r.try_get("checked_at").map_err(StoreError::backend)?;
            let status: Option<i32> = r.try_get("status").map_err(StoreError::backend)?;
            let healthy: bool = r.try_get("healthy").map_err(StoreError::backend)?;
            out.push((
                id as u64,
                LinkHealth {
                    checked_at: checked_at as u64,
                    status: status.map(|s| s as u16),
                    healthy,
                },
            ));
        }
        Ok(out)
    }

    async fn next_pixel_id(&self) -> Result<u64, StoreError> {
        let row = sqlx::query("SELECT nextval('quark_pixel_id_seq') AS id")
            .fetch_one(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        let id: i64 = row.try_get("id").map_err(StoreError::backend)?;
        Ok(id as u64)
    }

    async fn get_pixel(&self, id: u64) -> Result<Option<PixelConfig>, StoreError> {
        let row = sqlx::query(
            "SELECT id, provider, credentials, active, created FROM pixels WHERE id = $1",
        )
        .bind(id as i64)
        .fetch_optional(&self.pool)
        .await
        .map_err(StoreError::backend)?;
        match row {
            Some(r) => Ok(Some(row_to_pixel(&r)?)),
            None => Ok(None),
        }
    }

    async fn put_pixel(&self, config: &PixelConfig) -> Result<(), StoreError> {
        let credentials = serde_json::to_value(&config.credentials)?;
        sqlx::query(
            "INSERT INTO pixels (id, provider, credentials, active, created) VALUES ($1,$2,$3,$4,$5) \
             ON CONFLICT (id) DO UPDATE SET provider=$2, credentials=$3, active=$4, created=$5",
        )
        .bind(config.id as i64)
        .bind(provider_to_str(config.provider))
        .bind(&credentials)
        .bind(config.active)
        .bind(config.created as i64)
        .execute(&self.pool)
        .await
        .map_err(StoreError::backend)?;
        Ok(())
    }

    async fn delete_pixel(&self, id: u64) -> Result<bool, StoreError> {
        let res = sqlx::query("DELETE FROM pixels WHERE id = $1")
            .bind(id as i64)
            .execute(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        Ok(res.rows_affected() > 0)
    }

    async fn list_pixels(&self) -> Result<Vec<PixelConfig>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, provider, credentials, active, created FROM pixels ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(StoreError::backend)?;
        rows.iter().map(row_to_pixel).collect()
    }

    async fn get_wellknown(&self, name: &str) -> Result<Option<String>, StoreError> {
        let row = sqlx::query("SELECT body FROM wellknown_documents WHERE name = $1")
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        match row {
            Some(r) => {
                let body: String = r.try_get("body").map_err(StoreError::backend)?;
                Ok(Some(body))
            }
            None => Ok(None),
        }
    }

    async fn put_wellknown(&self, name: &str, body: &str) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO wellknown_documents (name, body) VALUES ($1,$2) \
             ON CONFLICT (name) DO UPDATE SET body=EXCLUDED.body",
        )
        .bind(name)
        .bind(body)
        .execute(&self.pool)
        .await
        .map_err(StoreError::backend)?;
        Ok(())
    }

    async fn delete_wellknown(&self, name: &str) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM wellknown_documents WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        Ok(())
    }

    async fn enqueue_deliveries(&self, rows: &[OutboxRow]) -> Result<(), StoreError> {
        if rows.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin().await.map_err(StoreError::backend)?;
        for row in rows {
            sqlx::query(
                "INSERT INTO webhook_deliveries (delivery_key, subscription_id, event_type, payload, created, next_attempt_at) \
                 VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT (delivery_key) DO NOTHING",
            )
            .bind(&row.delivery_key)
            .bind(row.subscription_id as i64)
            .bind(&row.event_type)
            .bind(&row.payload)
            .bind(row.created as i64)
            .bind(row.next_attempt_at as i64)
            .execute(&mut *tx)
            .await
            .map_err(StoreError::backend)?;
        }
        tx.commit().await.map_err(StoreError::backend)?;
        Ok(())
    }

    async fn claim_due_deliveries(
        &self,
        now: u64,
        limit: i64,
    ) -> Result<Vec<OutboxDelivery>, StoreError> {
        let lease_until = now.saturating_add(CLAIM_LEASE_SECS);
        let rows = sqlx::query(
            "UPDATE webhook_deliveries SET next_attempt_at = $1 \
             WHERE id IN ( \
                 SELECT id FROM webhook_deliveries \
                 WHERE dead = false AND delivered_at IS NULL AND next_attempt_at <= $2 \
                 ORDER BY next_attempt_at \
                 FOR UPDATE SKIP LOCKED \
                 LIMIT $3 \
             ) \
             RETURNING id, delivery_key, subscription_id, event_type, payload, attempts",
        )
        .bind(lease_until as i64)
        .bind(now as i64)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(StoreError::backend)?;
        rows.iter().map(row_to_delivery).collect()
    }

    async fn mark_delivered(&self, id: i64) -> Result<(), StoreError> {
        sqlx::query("UPDATE webhook_deliveries SET delivered_at = $1 WHERE id = $2")
            .bind(crate::now() as i64)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        Ok(())
    }

    async fn mark_retry(
        &self,
        id: i64,
        next_attempt_at: u64,
        attempts: u32,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE webhook_deliveries SET next_attempt_at = $1, attempts = $2 WHERE id = $3",
        )
        .bind(next_attempt_at as i64)
        .bind(attempts as i32)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(StoreError::backend)?;
        Ok(())
    }

    async fn mark_dead(&self, id: i64, attempts: u32) -> Result<(), StoreError> {
        sqlx::query("UPDATE webhook_deliveries SET dead = true, attempts = $1 WHERE id = $2")
            .bind(attempts as i32)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        Ok(())
    }
}

/// Flattens a batch DELTA (a fresh `Aggregates` that applied only this batch's
/// events) into `(dimension, bucket, count)` rows for `click_counters`. `total`
/// and `bots` use the empty bucket; every per_* map contributes one row per key.
/// Zero counts are skipped so an all-bot batch never writes empty per_* rows.
fn counter_rows(agg: &Aggregates) -> Vec<(&'static str, String, i64)> {
    let mut rows: Vec<(&'static str, String, i64)> = Vec::new();
    if agg.total > 0 {
        rows.push(("total", String::new(), agg.total as i64));
    }
    if agg.bots > 0 {
        rows.push(("bots", String::new(), agg.bots as i64));
    }
    for (dim, map) in [
        ("day", &agg.per_day),
        ("country", &agg.per_country),
        ("device", &agg.per_device),
        ("os", &agg.per_os),
        ("browser", &agg.per_browser),
        ("referer", &agg.per_referer),
        ("city", &agg.per_city),
        ("variant", &agg.per_variant),
    ] {
        for (bucket, count) in map {
            if *count > 0 {
                rows.push((dim, bucket.clone(), *count as i64));
            }
        }
    }
    rows
}

#[async_trait::async_trait]
impl AnalyticsSink for PostgresStore {
    async fn record_batch(&self, events: &[ClickEvent]) -> Result<(), StoreError> {
        if events.is_empty() {
            return Ok(());
        }
        use std::collections::BTreeMap;
        let mut by_id: BTreeMap<u64, Vec<&ClickEvent>> = BTreeMap::new();
        for e in events {
            by_id.entry(e.id).or_default().push(e);
        }
        let mut tx = self.pool.begin().await.map_err(StoreError::backend)?;
        for (id, evs) in by_id {
            let mut delta = Aggregates::default();
            for e in &evs {
                delta.apply(e);
            }
            for (dimension, bucket, count) in counter_rows(&delta) {
                sqlx::query(
                    "INSERT INTO click_counters (id, dimension, bucket, count) VALUES ($1,$2,$3,$4) \
                     ON CONFLICT (id, dimension, bucket) DO UPDATE SET count = click_counters.count + EXCLUDED.count",
                )
                .bind(id as i64)
                .bind(dimension)
                .bind(&bucket)
                .bind(count)
                .execute(&mut *tx)
                .await
                .map_err(StoreError::backend)?;
            }
            sqlx::query(
                "INSERT INTO stats_meta (id, first_ts, last_ts) VALUES ($1,$2,$3) \
                 ON CONFLICT (id) DO UPDATE SET first_ts = LEAST(stats_meta.first_ts, EXCLUDED.first_ts), last_ts = GREATEST(stats_meta.last_ts, EXCLUDED.last_ts)",
            )
            .bind(id as i64)
            .bind(delta.first_ts as i64)
            .bind(delta.last_ts as i64)
            .execute(&mut *tx)
            .await
            .map_err(StoreError::backend)?;
            for e in &evs {
                sqlx::query(
                    "INSERT INTO click_events (id, ts, referer, country, user_agent, city, variant, event_id) \
                     VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
                )
                .bind(id as i64)
                .bind(e.ts as i64)
                .bind(&e.referer)
                .bind(&e.country)
                .bind(&e.user_agent)
                .bind(&e.city)
                .bind(e.variant.map(|v| v as i32))
                .bind(&e.event_id)
                .execute(&mut *tx)
                .await
                .map_err(StoreError::backend)?;
            }
            sqlx::query(
                "DELETE FROM click_events WHERE id=$1 AND seq < \
                 (SELECT MIN(seq) FROM (SELECT seq FROM click_events WHERE id=$1 ORDER BY seq DESC LIMIT $2) t)",
            )
            .bind(id as i64)
            .bind(EVENTS_MAX as i64)
            .execute(&mut *tx)
            .await
            .map_err(StoreError::backend)?;
        }
        tx.commit().await.map_err(StoreError::backend)?;
        Ok(())
    }

    async fn stats(&self, id: u64) -> Result<Option<Stats>, StoreError> {
        let counter_rows =
            sqlx::query("SELECT dimension, bucket, count FROM click_counters WHERE id=$1")
                .bind(id as i64)
                .fetch_all(&self.pool)
                .await
                .map_err(StoreError::backend)?;
        let mut agg = Aggregates::default();
        for r in &counter_rows {
            let dimension: String = r.try_get("dimension").map_err(StoreError::backend)?;
            let bucket: String = r.try_get("bucket").map_err(StoreError::backend)?;
            let count: i64 = r.try_get("count").map_err(StoreError::backend)?;
            let count = count as u64;
            match dimension.as_str() {
                "total" => agg.total = count,
                "bots" => agg.bots = count,
                "day" => {
                    agg.per_day.insert(bucket, count);
                }
                "country" => {
                    agg.per_country.insert(bucket, count);
                }
                "device" => {
                    agg.per_device.insert(bucket, count);
                }
                "os" => {
                    agg.per_os.insert(bucket, count);
                }
                "browser" => {
                    agg.per_browser.insert(bucket, count);
                }
                "referer" => {
                    agg.per_referer.insert(bucket, count);
                }
                "city" => {
                    agg.per_city.insert(bucket, count);
                }
                "variant" => {
                    agg.per_variant.insert(bucket, count);
                }
                _ => {}
            }
        }
        let meta = sqlx::query("SELECT first_ts, last_ts FROM stats_meta WHERE id=$1")
            .bind(id as i64)
            .fetch_optional(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        if let Some(m) = &meta {
            let first_ts: i64 = m.try_get("first_ts").map_err(StoreError::backend)?;
            let last_ts: i64 = m.try_get("last_ts").map_err(StoreError::backend)?;
            agg.first_ts = first_ts as u64;
            agg.last_ts = last_ts as u64;
        }
        let event_rows = sqlx::query(
            "SELECT ts, referer, country, user_agent, city, variant, event_id FROM click_events \
             WHERE id=$1 ORDER BY seq DESC LIMIT $2",
        )
        .bind(id as i64)
        .bind(EVENTS_MAX as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(StoreError::backend)?;
        if counter_rows.is_empty() && event_rows.is_empty() {
            return Ok(None);
        }
        let mut recent: Vec<ClickEvent> = Vec::with_capacity(event_rows.len());
        for r in event_rows.iter().rev() {
            let ts: i64 = r.try_get("ts").map_err(StoreError::backend)?;
            let referer: Option<String> = r.try_get("referer").map_err(StoreError::backend)?;
            let country: Option<String> = r.try_get("country").map_err(StoreError::backend)?;
            let user_agent: Option<String> =
                r.try_get("user_agent").map_err(StoreError::backend)?;
            let city: Option<String> = r.try_get("city").map_err(StoreError::backend)?;
            let variant: Option<i32> = r.try_get("variant").map_err(StoreError::backend)?;
            let event_id: String = r.try_get("event_id").map_err(StoreError::backend)?;
            recent.push(ClickEvent {
                id,
                event_id,
                ts: ts as u64,
                referer,
                country,
                bot: is_bot(user_agent.as_deref()),
                user_agent,
                city,
                ip: None,
                fbc: None,
                variant: variant.map(|v| v as u32),
            });
        }
        Ok(Some(Stats {
            aggregates: agg,
            recent,
        }))
    }
}
