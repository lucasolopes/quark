use crate::analytics::{Aggregates, AnalyticsSink, ClickEvent, Stats, EVENTS_MAX};
use crate::store::{Record, Store, StoreError};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};

/// Chave do pg_advisory_lock que serializa a criação idempotente do schema entre instâncias.
const QUARK_SCHEMA_LOCK_ID: i64 = 727271;

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

    /// Cria o schema de forma idempotente. `CREATE TABLE/SEQUENCE IF NOT EXISTS`
    /// ainda pode colidir sob concorrência (várias conexões checam "não existe"
    /// e tentam criar ao mesmo tempo, batendo em unique constraints do catálogo
    /// do Postgres) — por isso serializamos com um advisory lock de sessão numa
    /// única conexão antes de rodar o DDL.
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
                "CREATE TABLE IF NOT EXISTS links (id BIGINT PRIMARY KEY, url TEXT NOT NULL, expiry BIGINT, created BIGINT NOT NULL)",
                "CREATE TABLE IF NOT EXISTS aliases (alias TEXT PRIMARY KEY, id BIGINT NOT NULL)",
                "CREATE TABLE IF NOT EXISTS stats (id BIGINT PRIMARY KEY, agg JSONB NOT NULL)",
                "CREATE TABLE IF NOT EXISTS events (id BIGINT PRIMARY KEY, recent JSONB NOT NULL)",
                "CREATE TABLE IF NOT EXISTS blocked_domains (domain TEXT PRIMARY KEY)",
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

    /// Uso em testes: zera todo o estado.
    pub async fn reset_for_tests(&self) -> Result<(), StoreError> {
        for q in [
            "TRUNCATE links, aliases, stats, events",
            "ALTER SEQUENCE quark_id_seq RESTART WITH 1",
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
        let row = sqlx::query("SELECT url, expiry, created FROM links WHERE id = $1")
            .bind(id as i64)
            .fetch_optional(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        match row {
            Some(r) => {
                let url: String = r.try_get("url").map_err(StoreError::backend)?;
                let expiry: Option<i64> = r.try_get("expiry").map_err(StoreError::backend)?;
                let created: i64 = r.try_get("created").map_err(StoreError::backend)?;
                Ok(Some(Record {
                    url,
                    expiry: expiry.map(|v| v as u64),
                    created: created as u64,
                }))
            }
            None => Ok(None),
        }
    }

    async fn put_link(&self, id: u64, rec: &Record) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO links (id, url, expiry, created) VALUES ($1,$2,$3,$4) \
             ON CONFLICT (id) DO UPDATE SET url=$2, expiry=$3, created=$4",
        )
        .bind(id as i64)
        .bind(&rec.url)
        .bind(rec.expiry.map(|v| v as i64))
        .bind(rec.created as i64)
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
            // alias já existe -> rollback (drop) e false
            return Ok(false);
        }
        sqlx::query(
            "INSERT INTO links (id, url, expiry, created) VALUES ($1,$2,$3,$4) \
             ON CONFLICT (id) DO UPDATE SET url=$2, expiry=$3, created=$4",
        )
        .bind(id as i64)
        .bind(&rec.url)
        .bind(rec.expiry.map(|v| v as i64))
        .bind(rec.created as i64)
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
            // Serializa read-modify-write concorrente sobre o mesmo id entre instâncias
            // (multi-node): sem isso, dois workers podem ler o mesmo snapshot de agg/recent,
            // recomputar e o segundo commit sobrescreve o primeiro (lost update). O lock
            // é escopado à transação (libera em commit/rollback) e funciona mesmo quando
            // as linhas de stats/events ainda não existem (diferente de SELECT ... FOR UPDATE).
            sqlx::query("SELECT pg_advisory_xact_lock($1)")
                .bind(id as i64)
                .execute(&mut *tx)
                .await
                .map_err(StoreError::backend)?;
            // agregados
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
            // eventos crus (ring)
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
        let recent: Vec<ClickEvent> = match row {
            Some(r) => {
                let v: serde_json::Value = r.try_get("recent").map_err(StoreError::backend)?;
                serde_json::from_value(v)?
            }
            None => Vec::new(),
        };
        Ok(Some(Stats {
            aggregates: agg,
            recent,
        }))
    }
}
