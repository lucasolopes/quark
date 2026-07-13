use crate::store::{Record, Store, StoreError};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};

pub struct PostgresStore {
    pool: PgPool,
}

impl PostgresStore {
    pub async fn open(url: &str) -> Result<PostgresStore, StoreError> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(url)
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;
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
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        sqlx::query("SELECT pg_advisory_lock(727271)")
            .execute(&mut *conn)
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;

        let result = async {
            for ddl in [
                "CREATE SEQUENCE IF NOT EXISTS quark_id_seq",
                "CREATE TABLE IF NOT EXISTS links (id BIGINT PRIMARY KEY, url TEXT NOT NULL, expiry BIGINT, created BIGINT NOT NULL)",
                "CREATE TABLE IF NOT EXISTS aliases (alias TEXT PRIMARY KEY, id BIGINT NOT NULL)",
                "CREATE TABLE IF NOT EXISTS stats (id BIGINT PRIMARY KEY, agg JSONB NOT NULL)",
                "CREATE TABLE IF NOT EXISTS events (id BIGINT PRIMARY KEY, recent JSONB NOT NULL)",
            ] {
                sqlx::query(ddl)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| StoreError::Backend(e.to_string()))?;
            }
            Ok(())
        }
        .await;

        sqlx::query("SELECT pg_advisory_unlock(727271)")
            .execute(&mut *conn)
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;

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
                .map_err(|e| StoreError::Backend(e.to_string()))?;
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
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        let id: i64 = row
            .try_get("id")
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        Ok(id as u64)
    }

    async fn get_link(&self, id: u64) -> Result<Option<Record>, StoreError> {
        let row = sqlx::query("SELECT url, expiry, created FROM links WHERE id = $1")
            .bind(id as i64)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        match row {
            Some(r) => {
                let url: String = r
                    .try_get("url")
                    .map_err(|e| StoreError::Backend(e.to_string()))?;
                let expiry: Option<i64> = r
                    .try_get("expiry")
                    .map_err(|e| StoreError::Backend(e.to_string()))?;
                let created: i64 = r
                    .try_get("created")
                    .map_err(|e| StoreError::Backend(e.to_string()))?;
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
        .map_err(|e| StoreError::Backend(e.to_string()))?;
        Ok(())
    }

    async fn get_alias(&self, alias: &str) -> Result<Option<u64>, StoreError> {
        let row = sqlx::query("SELECT id FROM aliases WHERE alias = $1")
            .bind(alias)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        match row {
            Some(r) => {
                let id: i64 = r
                    .try_get("id")
                    .map_err(|e| StoreError::Backend(e.to_string()))?;
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
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        let res = sqlx::query(
            "INSERT INTO aliases (alias, id) VALUES ($1,$2) ON CONFLICT (alias) DO NOTHING",
        )
        .bind(alias)
        .bind(id as i64)
        .execute(&mut *tx)
        .await
        .map_err(|e| StoreError::Backend(e.to_string()))?;
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
        .map_err(|e| StoreError::Backend(e.to_string()))?;
        tx.commit()
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        Ok(true)
    }
}
