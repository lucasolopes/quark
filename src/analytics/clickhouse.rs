use crate::analytics::{device_from_ua, Aggregates, AnalyticsSink, ClickEvent, Stats, EVENTS_MAX};
use crate::store::StoreError;
use clickhouse::Row;
use serde::{Deserialize, Serialize};

#[derive(Row, Serialize)]
struct ClickRow<'a> {
    id: u64,
    ts: u64,
    country: &'a str,
    device: &'a str,
    referer: &'a str,
    /// A/B variant index served, or -1 when the link has no variants.
    variant: i32,
}

#[derive(Row, Deserialize)]
struct Totals {
    total: u64,
    first_ts: u64,
    last_ts: u64,
}
#[derive(Row, Deserialize)]
struct Kv {
    k: String,
    c: u64,
}
#[derive(Row, Deserialize)]
struct RecentRow {
    ts: u64,
    country: String,
    #[allow(dead_code)]
    device: String,
    referer: String,
    variant: i32,
}

/// Empty string becomes `None`, otherwise `Some(s)`. Shortens the empty-field pattern.
/// Takes ownership: moves along the reconstruction path (no extra allocation).
fn non_empty(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Extracts (endpoint scheme://host[:port], user, password, database) from a URL.
#[allow(clippy::type_complexity)]
fn parse_ch_url(
    raw: &str,
) -> Result<(String, Option<String>, Option<String>, Option<String>), StoreError> {
    let u = url::Url::parse(raw)
        .map_err(|e| StoreError::Backend(format!("invalid ClickHouse URL: {e}")))?;
    let scheme = u.scheme();
    let host = u
        .host_str()
        .ok_or_else(|| StoreError::Backend("ClickHouse URL missing host".into()))?;
    let endpoint = match u.port() {
        Some(p) => format!("{scheme}://{host}:{p}"),
        None => format!("{scheme}://{host}"),
    };
    let user = non_empty(u.username().to_string());
    let pass = u.password().map(|s| s.to_string());
    let db = non_empty(u.path().trim_start_matches('/').to_string());
    Ok((endpoint, user, pass, db))
}

pub struct ClickHouseSink {
    client: clickhouse::Client,
}

impl ClickHouseSink {
    pub async fn open(url: &str) -> Result<ClickHouseSink, StoreError> {
        let (endpoint, user, pass, db) = parse_ch_url(url)?;
        let mut client = clickhouse::Client::default().with_url(endpoint);
        if let Some(u) = user {
            client = client.with_user(u);
        }
        if let Some(p) = pass {
            client = client.with_password(p);
        }
        if let Some(d) = db {
            client = client.with_database(d);
        }
        let s = ClickHouseSink { client };
        s.init_schema().await?;
        Ok(s)
    }

    async fn init_schema(&self) -> Result<(), StoreError> {
        self.client
            .query(
                "CREATE TABLE IF NOT EXISTS clicks (id UInt64, ts UInt64, country String, device String, referer String, variant Int32 DEFAULT -1) ENGINE = MergeTree ORDER BY (id, ts)"
            )
            .execute()
            .await
            .map_err(StoreError::backend)?;
        // Idempotent migration for tables created before the `variant` column
        // existed: `CREATE TABLE IF NOT EXISTS` above is a no-op on an
        // already-existing table, so the column must be added separately.
        self.client
            .query("ALTER TABLE clicks ADD COLUMN IF NOT EXISTS variant Int32 DEFAULT -1")
            .execute()
            .await
            .map_err(StoreError::backend)
    }

    /// Used in tests: resets all state.
    pub async fn reset_for_tests(&self) -> Result<(), StoreError> {
        self.client
            .query("TRUNCATE TABLE IF EXISTS clicks")
            .execute()
            .await
            .map_err(StoreError::backend)
    }
}

#[async_trait::async_trait]
impl AnalyticsSink for ClickHouseSink {
    async fn record_batch(&self, events: &[ClickEvent]) -> Result<(), StoreError> {
        if events.is_empty() {
            return Ok(());
        }
        let mut insert = self.client.insert("clicks").map_err(StoreError::backend)?;
        for e in events {
            let device = device_from_ua(e.user_agent.as_deref());
            let row = ClickRow {
                id: e.id,
                ts: e.ts,
                country: e.country.as_deref().unwrap_or(""),
                device,
                referer: e.referer.as_deref().unwrap_or(""),
                variant: e.variant.map(|v| v as i32).unwrap_or(-1),
            };
            insert.write(&row).await.map_err(StoreError::backend)?;
        }
        insert.end().await.map_err(StoreError::backend)?;
        Ok(())
    }

    async fn stats(&self, id: u64) -> Result<Option<Stats>, StoreError> {
        let totals: Totals = self
            .client
            .query("SELECT count() AS total, min(ts) AS first_ts, max(ts) AS last_ts FROM clicks WHERE id = ?")
            .bind(id)
            .fetch_one()
            .await
            .map_err(StoreError::backend)?;
        if totals.total == 0 {
            return Ok(None);
        }

        let mut agg = Aggregates {
            total: totals.total,
            first_ts: totals.first_ts,
            last_ts: totals.last_ts,
            ..Default::default()
        };

        let per_day: Vec<Kv> = self
            .client
            .query(
                "SELECT formatDateTime(toDateTime(ts,'UTC'),'%F') AS k, count() AS c FROM clicks WHERE id = ? GROUP BY k",
            )
            .bind(id)
            .fetch_all()
            .await
            .map_err(StoreError::backend)?;
        for kv in per_day {
            agg.per_day.insert(kv.k, kv.c);
        }

        let per_country: Vec<Kv> = self
            .client
            .query("SELECT country AS k, count() AS c FROM clicks WHERE id = ? GROUP BY k")
            .bind(id)
            .fetch_all()
            .await
            .map_err(StoreError::backend)?;
        for kv in per_country {
            if !kv.k.is_empty() {
                agg.per_country.insert(kv.k, kv.c);
            }
        }

        let per_device: Vec<Kv> = self
            .client
            .query("SELECT device AS k, count() AS c FROM clicks WHERE id = ? GROUP BY k")
            .bind(id)
            .fetch_all()
            .await
            .map_err(StoreError::backend)?;
        for kv in per_device {
            agg.per_device.insert(kv.k, kv.c);
        }

        let per_variant: Vec<Kv> = self
            .client
            .query(
                "SELECT toString(variant) AS k, count() AS c FROM clicks WHERE id = ? AND variant >= 0 GROUP BY k",
            )
            .bind(id)
            .fetch_all()
            .await
            .map_err(StoreError::backend)?;
        for kv in per_variant {
            agg.per_variant.insert(kv.k, kv.c);
        }

        let mut recent_rows: Vec<RecentRow> = self
            .client
            .query("SELECT ts, country, device, referer, variant FROM clicks WHERE id = ? ORDER BY ts DESC LIMIT ?")
            .bind(id)
            .bind(EVENTS_MAX as u64)
            .fetch_all()
            .await
            .map_err(StoreError::backend)?;
        recent_rows.reverse();
        let recent = recent_rows
            .into_iter()
            .map(|r| ClickEvent {
                id,
                ts: r.ts,
                referer: non_empty(r.referer),
                country: non_empty(r.country),
                user_agent: None,
                variant: if r.variant >= 0 {
                    Some(r.variant as u32)
                } else {
                    None
                },
            })
            .collect();

        Ok(Some(Stats {
            aggregates: agg,
            recent,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::parse_ch_url;

    #[test]
    fn parse_ch_url_without_credentials() {
        let (endpoint, user, pass, db) = parse_ch_url("http://127.0.0.1:8123").unwrap();
        assert_eq!(endpoint, "http://127.0.0.1:8123");
        assert_eq!(user, None);
        assert_eq!(pass, None);
        assert_eq!(db, None);
    }

    #[test]
    fn parse_ch_url_with_credentials_and_database() {
        let (endpoint, user, pass, db) =
            parse_ch_url("http://user:pass@host:8123/analytics").unwrap();
        assert_eq!(endpoint, "http://host:8123");
        assert_eq!(user, Some("user".to_string()));
        assert_eq!(pass, Some("pass".to_string()));
        assert_eq!(db, Some("analytics".to_string()));
    }

    #[test]
    fn parse_ch_url_without_port() {
        let (endpoint, user, pass, db) = parse_ch_url("https://host").unwrap();
        assert_eq!(endpoint, "https://host");
        assert_eq!(user, None);
        assert_eq!(pass, None);
        assert_eq!(db, None);
    }
}
