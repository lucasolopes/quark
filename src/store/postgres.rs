use crate::analytics::{is_bot, Aggregates, AnalyticsSink, ClickEvent, Stats, EVENTS_MAX};
use crate::auth::ApiToken;
use crate::store::{Record, Store, StoreError};
use crate::webhooks::{SubscriptionKind, WebhookSubscription};
use sqlx::postgres::{PgPoolOptions, PgRow};
use sqlx::{PgPool, Row};

/// Key of the pg_advisory_lock that serializes idempotent schema creation across instances.
const QUARK_SCHEMA_LOCK_ID: i64 = 727271;

/// Escapes `LIKE`/`ILIKE` wildcards (default escape char = `\`) so that the
/// user's term is treated literally. Order matters: escape `\` first.
fn like_escape(q: &str) -> String {
    q.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Maps a `links` row (id, url, expiry, created, tags) into `(id, Record)`.
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
    Ok((
        id as u64,
        Record {
            url,
            expiry: expiry.map(|v| v as u64),
            created: created as u64,
            tags,
            max_visits: max_visits.map(|v| v as u32),
            rules,
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
                "CREATE TABLE IF NOT EXISTS aliases (alias TEXT PRIMARY KEY, id BIGINT NOT NULL)",
                "CREATE TABLE IF NOT EXISTS stats (id BIGINT PRIMARY KEY, agg JSONB NOT NULL)",
                "CREATE TABLE IF NOT EXISTS events (id BIGINT PRIMARY KEY, recent JSONB NOT NULL)",
                "CREATE TABLE IF NOT EXISTS blocked_domains (domain TEXT PRIMARY KEY)",
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
            "TRUNCATE links, aliases, stats, events, webhooks, api_tokens",
            "ALTER SEQUENCE quark_id_seq RESTART WITH 1",
            "ALTER SEQUENCE quark_webhook_id_seq RESTART WITH 1",
            "ALTER SEQUENCE quark_api_token_id_seq RESTART WITH 1",
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
            "SELECT url, expiry, created, tags, max_visits, rules FROM links WHERE id = $1",
        )
        .bind(id as i64)
        .fetch_optional(&self.pool)
        .await
        .map_err(StoreError::backend)?;
        match row {
            Some(r) => {
                let url: String = r.try_get("url").map_err(StoreError::backend)?;
                let expiry: Option<i64> = r.try_get("expiry").map_err(StoreError::backend)?;
                let created: i64 = r.try_get("created").map_err(StoreError::backend)?;
                let tags: serde_json::Value = r.try_get("tags").map_err(StoreError::backend)?;
                let tags: Vec<String> = serde_json::from_value(tags)?;
                let max_visits: Option<i64> =
                    r.try_get("max_visits").map_err(StoreError::backend)?;
                let rules: serde_json::Value = r.try_get("rules").map_err(StoreError::backend)?;
                let rules: Vec<crate::store::Rule> = serde_json::from_value(rules)?;
                Ok(Some(Record {
                    url,
                    expiry: expiry.map(|v| v as u64),
                    created: created as u64,
                    tags,
                    max_visits: max_visits.map(|v| v as u32),
                    rules,
                }))
            }
            None => Ok(None),
        }
    }

    async fn put_link(&self, id: u64, rec: &Record) -> Result<(), StoreError> {
        let tags = serde_json::to_value(&rec.tags)?;
        let rules = serde_json::to_value(&rec.rules)?;
        sqlx::query(
            "INSERT INTO links (id, url, expiry, created, tags, max_visits, rules) VALUES ($1,$2,$3,$4,$5,$6,$7) \
             ON CONFLICT (id) DO UPDATE SET url=$2, expiry=$3, created=$4, tags=$5, max_visits=$6, rules=$7",
        )
        .bind(id as i64)
        .bind(&rec.url)
        .bind(rec.expiry.map(|v| v as i64))
        .bind(rec.created as i64)
        .bind(&tags)
        .bind(rec.max_visits.map(|v| v as i64))
        .bind(&rules)
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
        sqlx::query(
            "INSERT INTO links (id, url, expiry, created, tags, max_visits, rules) VALUES ($1,$2,$3,$4,$5,$6,$7) \
             ON CONFLICT (id) DO UPDATE SET url=$2, expiry=$3, created=$4, tags=$5, max_visits=$6, rules=$7",
        )
        .bind(id as i64)
        .bind(&rec.url)
        .bind(rec.expiry.map(|v| v as i64))
        .bind(rec.created as i64)
        .bind(&tags)
        .bind(rec.max_visits.map(|v| v as i64))
        .bind(&rules)
        .execute(&mut *tx)
        .await
        .map_err(StoreError::backend)?;
        tx.commit().await.map_err(StoreError::backend)?;
        Ok(true)
    }

    async fn add_blocked_domain(&self, domain: &str) -> Result<(), StoreError> {
        let d = domain.trim().to_lowercase();
        sqlx::query("INSERT INTO blocked_domains (domain) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(&d)
            .execute(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        Ok(())
    }

    async fn remove_blocked_domain(&self, domain: &str) -> Result<(), StoreError> {
        let d = domain.trim().to_lowercase();
        sqlx::query("DELETE FROM blocked_domains WHERE domain = $1")
            .bind(&d)
            .execute(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        Ok(())
    }

    async fn list_blocked_domains(&self) -> Result<Vec<String>, StoreError> {
        let rows = sqlx::query("SELECT domain FROM blocked_domains")
            .fetch_all(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        rows.iter()
            .map(|r| {
                r.try_get::<String, _>("domain")
                    .map_err(StoreError::backend)
            })
            .collect()
    }

    async fn list_links(
        &self,
        after: Option<u64>,
        limit: usize,
        tag: Option<&str>,
    ) -> Result<Vec<(u64, Record)>, StoreError> {
        let tag_json = tag.map(|t| serde_json::json!([t]));
        let rows = sqlx::query(
            "SELECT id, url, expiry, created, tags, max_visits, rules FROM links \
             WHERE ($1::bigint IS NULL OR id > $1) \
               AND ($2::jsonb IS NULL OR tags @> $2) \
             ORDER BY id LIMIT $3",
        )
        .bind(after.map(|a| a as i64))
        .bind(&tag_json)
        .bind(limit as i64)
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
    ) -> Result<Vec<(u64, Record)>, StoreError> {
        let pattern = format!("%{}%", like_escape(q));
        let tag_json = tag.map(|t| serde_json::json!([t]));
        let rows = sqlx::query(
            "SELECT DISTINCT l.id, l.url, l.expiry, l.created, l.tags, l.max_visits, l.rules \
             FROM links l LEFT JOIN aliases a ON a.id = l.id \
             WHERE ($1::bigint IS NULL OR l.id > $1) \
               AND (l.url ILIKE $2 OR a.alias ILIKE $2) \
               AND ($3::jsonb IS NULL OR l.tags @> $3) \
             ORDER BY l.id LIMIT $4",
        )
        .bind(after.map(|a| a as i64))
        .bind(&pattern)
        .bind(&tag_json)
        .bind(limit as i64)
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

    async fn list_tags(&self) -> Result<Vec<String>, StoreError> {
        let rows = sqlx::query(
            "SELECT DISTINCT jsonb_array_elements_text(tags) AS tag FROM links ORDER BY tag",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(StoreError::backend)?;
        rows.iter()
            .map(|r| r.try_get::<String, _>("tag").map_err(StoreError::backend))
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
            sqlx::query("SELECT pg_advisory_xact_lock($1)")
                .bind(id as i64)
                .execute(&mut *tx)
                .await
                .map_err(StoreError::backend)?;
            let row = sqlx::query("SELECT agg FROM stats WHERE id=$1")
                .bind(id as i64)
                .fetch_optional(&mut *tx)
                .await
                .map_err(StoreError::backend)?;
            let mut agg: Aggregates = match row {
                Some(r) => {
                    let v: serde_json::Value = r.try_get("agg").map_err(StoreError::backend)?;
                    serde_json::from_value(v)?
                }
                None => Aggregates::default(),
            };
            for e in &evs {
                agg.apply(e);
            }
            let aggv = serde_json::to_value(&agg)?;
            sqlx::query(
                "INSERT INTO stats (id, agg) VALUES ($1,$2) ON CONFLICT (id) DO UPDATE SET agg=$2",
            )
            .bind(id as i64)
            .bind(&aggv)
            .execute(&mut *tx)
            .await
            .map_err(StoreError::backend)?;
            let row = sqlx::query("SELECT recent FROM events WHERE id=$1")
                .bind(id as i64)
                .fetch_optional(&mut *tx)
                .await
                .map_err(StoreError::backend)?;
            let mut recent: Vec<ClickEvent> = match row {
                Some(r) => {
                    let v: serde_json::Value = r.try_get("recent").map_err(StoreError::backend)?;
                    serde_json::from_value(v)?
                }
                None => Vec::new(),
            };
            for e in &evs {
                recent.push((*e).clone());
            }
            if recent.len() > EVENTS_MAX {
                let d = recent.len() - EVENTS_MAX;
                recent.drain(0..d);
            }
            let recv = serde_json::to_value(&recent)?;
            sqlx::query(
                "INSERT INTO events (id, recent) VALUES ($1,$2) ON CONFLICT (id) DO UPDATE SET recent=$2",
            )
            .bind(id as i64)
            .bind(&recv)
            .execute(&mut *tx)
            .await
            .map_err(StoreError::backend)?;
        }
        tx.commit().await.map_err(StoreError::backend)?;
        Ok(())
    }

    async fn stats(&self, id: u64) -> Result<Option<Stats>, StoreError> {
        let row = sqlx::query("SELECT agg FROM stats WHERE id=$1")
            .bind(id as i64)
            .fetch_optional(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        let agg: Aggregates = match row {
            Some(r) => {
                let v: serde_json::Value = r.try_get("agg").map_err(StoreError::backend)?;
                serde_json::from_value(v)?
            }
            None => return Ok(None),
        };
        let row = sqlx::query("SELECT recent FROM events WHERE id=$1")
            .bind(id as i64)
            .fetch_optional(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        let mut recent: Vec<ClickEvent> = match row {
            Some(r) => {
                let v: serde_json::Value = r.try_get("recent").map_err(StoreError::backend)?;
                serde_json::from_value(v)?
            }
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
