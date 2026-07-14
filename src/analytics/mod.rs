use crate::store::StoreError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::mpsc::Receiver;

pub mod clickhouse;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClickEvent {
    pub id: u64,
    pub ts: u64,
    pub referer: Option<String>,
    pub country: Option<String>,
    pub user_agent: Option<String>,
    /// Index of the A/B variant served for this click; `None` when the link
    /// has no variants (the common case).
    #[serde(default)]
    pub variant: Option<u32>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Aggregates {
    pub total: u64,
    pub first_ts: u64,
    pub last_ts: u64,
    pub per_day: BTreeMap<String, u64>,
    pub per_country: BTreeMap<String, u64>,
    pub per_device: BTreeMap<String, u64>,
    /// Clicks per variant index (stringified), keyed the same way the UI
    /// looks them up against `Record.variants`.
    #[serde(default)]
    pub per_variant: BTreeMap<String, u64>,
}

impl Aggregates {
    pub fn apply(&mut self, ev: &ClickEvent) {
        self.total += 1;
        if self.total == 1 || ev.ts < self.first_ts {
            self.first_ts = ev.ts;
        }
        if ev.ts > self.last_ts {
            self.last_ts = ev.ts;
        }
        *self.per_day.entry(day_bucket(ev.ts)).or_insert(0) += 1;
        if let Some(c) = &ev.country {
            *self.per_country.entry(c.clone()).or_insert(0) += 1;
        }
        let dev = device_from_ua(ev.user_agent.as_deref());
        *self.per_device.entry(dev.to_string()).or_insert(0) += 1;
        if let Some(variant) = ev.variant {
            *self.per_variant.entry(variant.to_string()).or_insert(0) += 1;
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub aggregates: Aggregates,
    pub recent: Vec<ClickEvent>,
}

/// Maximum number of raw events retained per id (circular retention).
pub const EVENTS_MAX: usize = 1000;

/// Lightweight device heuristic from the User-Agent (no external dep).
pub fn device_from_ua(ua: Option<&str>) -> &'static str {
    match ua {
        Some(s) => {
            let s = s.to_ascii_lowercase();
            if s.contains("iphone") || s.contains("android") || s.contains("mobile") {
                "Mobile"
            } else if s.contains("windows")
                || s.contains("macintosh")
                || s.contains("x11")
                || s.contains("linux")
            {
                "Desktop"
            } else {
                "Other"
            }
        }
        None => "Other",
    }
}

/// YYYY-MM-DD (UTC) from epoch secs, via day arithmetic (no chrono).
pub fn day_bucket(ts: u64) -> String {
    let days = (ts / 86_400) as i64;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[async_trait::async_trait]
pub trait AnalyticsSink: Send + Sync + 'static {
    async fn record_batch(&self, events: &[ClickEvent]) -> Result<(), StoreError>;
    async fn stats(&self, id: u64) -> Result<Option<Stats>, StoreError>;
}

/// Batch size that triggers an immediate flush (in addition to the 5s timer).
pub const BATCH: usize = 500;

/// Background worker: accumulates `ClickEvent`s from the channel and flushes
/// to the sink when the buffer reaches `BATCH`, every 5s, or when the channel
/// closes (drains and exits).
pub fn spawn_worker(
    mut rx: Receiver<ClickEvent>,
    sink: Arc<dyn AnalyticsSink>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut buf: Vec<ClickEvent> = Vec::with_capacity(BATCH);
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(5));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                maybe = rx.recv() => {
                    match maybe {
                        Some(ev) => {
                            buf.push(ev);
                            if buf.len() >= BATCH {
                                flush(&sink, &mut buf).await;
                            }
                        }
                        None => {
                            flush(&sink, &mut buf).await;
                            break;
                        }
                    }
                }
                _ = ticker.tick() => {
                    flush(&sink, &mut buf).await;
                }
            }
        }
    })
}

async fn flush(sink: &Arc<dyn AnalyticsSink>, buf: &mut Vec<ClickEvent>) {
    if buf.is_empty() {
        return;
    }
    if let Err(e) = sink.record_batch(buf).await {
        eprintln!(
            "{}",
            serde_json::json!({"analytics_flush_error": e.to_string()})
        );
    }
    buf.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(id: u64, ts: u64, country: &str, ua: &str) -> ClickEvent {
        ClickEvent {
            id,
            ts,
            referer: None,
            country: Some(country.into()),
            user_agent: Some(ua.into()),
            variant: None,
        }
    }

    #[test]
    fn aggregates_total_day_country_device() {
        let mut a = Aggregates::default();
        a.apply(&ev(1, 1_752_300_000, "BR", "Mozilla/5.0 (iPhone)"));
        a.apply(&ev(1, 1_752_300_050, "BR", "Mozilla/5.0 (Windows NT 10.0)"));
        a.apply(&ev(1, 1_752_400_000, "US", "curl/8.0"));
        assert_eq!(a.total, 3);
        assert_eq!(a.first_ts, 1_752_300_000);
        assert_eq!(a.last_ts, 1_752_400_000);
        assert_eq!(a.per_country.get("BR"), Some(&2));
        assert_eq!(a.per_country.get("US"), Some(&1));
        assert_eq!(a.per_device.get("Mobile"), Some(&1));
        assert_eq!(a.per_device.get("Desktop"), Some(&1));
        assert_eq!(a.per_device.get("Other"), Some(&1));
        assert_eq!(a.per_day.values().sum::<u64>(), 3);
    }

    #[test]
    fn device_heuristic() {
        assert_eq!(
            device_from_ua(Some("Mozilla/5.0 (iPhone; CPU iPhone OS)")),
            "Mobile"
        );
        assert_eq!(
            device_from_ua(Some("Mozilla/5.0 (Linux; Android 14)")),
            "Mobile"
        );
        assert_eq!(
            device_from_ua(Some("Mozilla/5.0 (Windows NT 10.0; Win64)")),
            "Desktop"
        );
        assert_eq!(
            device_from_ua(Some("Mozilla/5.0 (Macintosh; Intel Mac OS X)")),
            "Desktop"
        );
        assert_eq!(device_from_ua(Some("curl/8.0")), "Other");
        assert_eq!(device_from_ua(None), "Other");
    }

    #[test]
    fn day_bucket_known_dates() {
        assert_eq!(day_bucket(0), "1970-01-01");
        assert_eq!(day_bucket(1_735_689_600), "2025-01-01");
        assert_eq!(day_bucket(1_735_689_600 + 86_400), "2025-01-02");
    }

    #[tokio::test]
    async fn worker_drains_and_writes_on_channel_close() {
        let dir = tempfile::tempdir().unwrap();
        let (_store, sink) = crate::store::open_backends(dir.path()).await.unwrap();
        let (tx, rx) = tokio::sync::mpsc::channel::<ClickEvent>(1000);
        let handle = spawn_worker(rx, sink.clone());

        for i in 0..250u64 {
            tx.send(ClickEvent {
                id: 5,
                ts: 1_752_300_000 + i,
                referer: None,
                country: Some("BR".into()),
                user_agent: Some("iPhone".into()),
                variant: None,
            })
            .await
            .unwrap();
        }
        drop(tx);
        handle.await.unwrap();

        let s = sink.stats(5).await.unwrap().unwrap();
        assert_eq!(s.aggregates.total, 250);
        assert_eq!(s.recent.len(), 250);
    }

    #[test]
    fn first_ts_handles_epoch_zero() {
        let mut a = Aggregates::default();
        a.apply(&ClickEvent {
            id: 1,
            ts: 0,
            referer: None,
            country: None,
            user_agent: None,
            variant: None,
        });
        a.apply(&ClickEvent {
            id: 1,
            ts: 5_000_000_000,
            referer: None,
            country: None,
            user_agent: None,
            variant: None,
        });
        assert_eq!(a.first_ts, 0);
        assert_eq!(a.last_ts, 5_000_000_000);
    }

    #[test]
    fn apply_increments_per_variant_only_when_some() {
        let mut a = Aggregates::default();
        a.apply(&ClickEvent {
            id: 1,
            ts: 1,
            referer: None,
            country: None,
            user_agent: None,
            variant: Some(0),
        });
        a.apply(&ClickEvent {
            id: 1,
            ts: 2,
            referer: None,
            country: None,
            user_agent: None,
            variant: Some(0),
        });
        a.apply(&ClickEvent {
            id: 1,
            ts: 3,
            referer: None,
            country: None,
            user_agent: None,
            variant: Some(1),
        });
        a.apply(&ClickEvent {
            id: 1,
            ts: 4,
            referer: None,
            country: None,
            user_agent: None,
            variant: None,
        });
        assert_eq!(a.per_variant.get("0"), Some(&2));
        assert_eq!(a.per_variant.get("1"), Some(&1));
        assert_eq!(a.total, 4);
    }

    #[test]
    fn click_event_without_variant_field_deserializes_to_none() {
        let old = r#"{"id":1,"ts":1,"referer":null,"country":null,"user_agent":null}"#;
        let ev: ClickEvent = serde_json::from_str(old).unwrap();
        assert_eq!(ev.variant, None);
    }

    #[test]
    fn aggregates_without_per_variant_field_deserializes_to_empty_map() {
        let old =
            r#"{"total":1,"first_ts":1,"last_ts":1,"per_day":{},"per_country":{},"per_device":{}}"#;
        let a: Aggregates = serde_json::from_str(old).unwrap();
        assert!(a.per_variant.is_empty());
    }
}
