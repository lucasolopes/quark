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
    #[allow(dead_code)] // selecionado para casar a ordem de colunas; não usado na reconstrução
    device: String,
    referer: String,
}

pub struct ClickHouseSink {
    client: clickhouse::Client,
}

impl ClickHouseSink {
    pub async fn open(url: &str) -> Result<ClickHouseSink, StoreError> {
        let client = clickhouse::Client::default().with_url(url);
        let s = ClickHouseSink { client };
        s.init_schema().await?;
        Ok(s)
    }

    async fn init_schema(&self) -> Result<(), StoreError> {
        self.client
            .query(
                "CREATE TABLE IF NOT EXISTS clicks (id UInt64, ts UInt64, country String, device String, referer String) ENGINE = MergeTree ORDER BY (id, ts)"
            )
            .execute()
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))
    }

    /// Uso em testes: zera todo o estado.
    pub async fn reset_for_tests(&self) -> Result<(), StoreError> {
        self.client
            .query("TRUNCATE TABLE IF EXISTS clicks")
            .execute()
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))
    }
}

#[async_trait::async_trait]
impl AnalyticsSink for ClickHouseSink {
    async fn record_batch(&self, events: &[ClickEvent]) -> Result<(), StoreError> {
        if events.is_empty() {
            return Ok(());
        }
        let mut insert = self
            .client
            .insert("clicks")
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        for e in events {
            let device = device_from_ua(e.user_agent.as_deref());
            let row = ClickRow {
                id: e.id,
                ts: e.ts,
                country: e.country.as_deref().unwrap_or(""),
                device,
                referer: e.referer.as_deref().unwrap_or(""),
            };
            insert
                .write(&row)
                .await
                .map_err(|e| StoreError::Backend(e.to_string()))?;
        }
        insert
            .end()
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        Ok(())
    }

    async fn stats(&self, id: u64) -> Result<Option<Stats>, StoreError> {
        let totals: Totals = self
            .client
            .query("SELECT count() AS total, min(ts) AS first_ts, max(ts) AS last_ts FROM clicks WHERE id = ?")
            .bind(id)
            .fetch_one()
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;
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
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        for kv in per_day {
            agg.per_day.insert(kv.k, kv.c);
        }

        let per_country: Vec<Kv> = self
            .client
            .query("SELECT country AS k, count() AS c FROM clicks WHERE id = ? GROUP BY k")
            .bind(id)
            .fetch_all()
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;
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
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        for kv in per_device {
            agg.per_device.insert(kv.k, kv.c);
        }

        let mut recent_rows: Vec<RecentRow> = self
            .client
            .query("SELECT ts, country, device, referer FROM clicks WHERE id = ? ORDER BY ts DESC LIMIT ?")
            .bind(id)
            .bind(EVENTS_MAX as u64)
            .fetch_all()
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        recent_rows.reverse(); // cronológico
        let recent = recent_rows
            .into_iter()
            .map(|r| ClickEvent {
                id,
                ts: r.ts,
                referer: if r.referer.is_empty() {
                    None
                } else {
                    Some(r.referer)
                },
                country: if r.country.is_empty() {
                    None
                } else {
                    Some(r.country)
                },
                user_agent: None, // ClickHouse guarda device, não o UA cru (fidelidade documentada)
            })
            .collect();

        Ok(Some(Stats {
            aggregates: agg,
            recent,
        }))
    }
}
