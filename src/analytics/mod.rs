use crate::pixel::{self, PixelBases, PixelConfig};
use crate::store::{Store, StoreError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::Receiver;

pub mod clickhouse;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClickEvent {
    pub id: u64,
    pub ts: u64,
    pub referer: Option<String>,
    pub country: Option<String>,
    pub user_agent: Option<String>,
    /// Captured only to forward server-side conversions (Meta CAPI user_data).
    /// `serde(skip)` keeps them in memory for the worker but out of the
    /// persisted recent-events buffer, so the raw IP never lands on disk.
    #[serde(skip)]
    pub ip: Option<String>,
    #[serde(skip)]
    pub fbc: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Aggregates {
    pub total: u64,
    pub first_ts: u64,
    pub last_ts: u64,
    pub per_day: BTreeMap<String, u64>,
    pub per_country: BTreeMap<String, u64>,
    pub per_device: BTreeMap<String, u64>,
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

/// How long a pixel-snapshot refresh (`store.list_pixels()`) is allowed to
/// run before it's abandoned in favor of the previous snapshot (fail-open:
/// a wedged store must never stall the worker).
const PIXEL_SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(3);

/// Background worker: accumulates `ClickEvent`s from the channel and flushes
/// to the sink when the buffer reaches `BATCH`, every 5s, or when the channel
/// closes (drains and exits). Each flush also forwards the batch to every
/// active pixel config (server-side conversion forwarding, roadmap #14):
/// async only, off the redirect hot path, and fail-open (a provider error is
/// only logged, never propagated to the caller or the sink write).
///
/// The pixel config list is read from `store` only once up front and then on
/// every 5s tick — never on the flush path itself (mirrors the webhook
/// worker's subscription-snapshot pattern, #1). This means a wedged store
/// (e.g. an exhausted Postgres pool) can never stall `flush`/forward and
/// back up the bounded analytics channel: the worker keeps forwarding to the
/// last-known-good snapshot, and a refresh that fails or times out just
/// keeps that snapshot (fail-open) instead of blocking.
///
/// `client` is the shared `reqwest::Client` used for provider calls (built
/// with no redirects and a timeout by the caller); `key` derives the real
/// short code used as `link_code` in the forwarded payload; `bases` are the
/// provider hosts (fixed in production, injectable in tests).
pub fn spawn_worker(
    mut rx: Receiver<ClickEvent>,
    sink: Arc<dyn AnalyticsSink>,
    store: Arc<dyn Store>,
    client: reqwest::Client,
    key: u64,
    bases: PixelBases,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut buf: Vec<ClickEvent> = Vec::with_capacity(BATCH);
        let mut pixels: Vec<PixelConfig> = Vec::new();
        refresh_pixel_snapshot(&store, &mut pixels).await;
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(5));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                maybe = rx.recv() => {
                    match maybe {
                        Some(ev) => {
                            buf.push(ev);
                            if buf.len() >= BATCH {
                                flush(&sink, &mut buf, &pixels, &client, key, &bases).await;
                            }
                        }
                        None => {
                            flush(&sink, &mut buf, &pixels, &client, key, &bases).await;
                            break;
                        }
                    }
                }
                _ = ticker.tick() => {
                    refresh_pixel_snapshot(&store, &mut pixels).await;
                    flush(&sink, &mut buf, &pixels, &client, key, &bases).await;
                }
            }
        }
    })
}

/// Refreshes the cached pixel-config snapshot from `store`, bounded by
/// `PIXEL_SNAPSHOT_TIMEOUT`. Fail-open: on a store error or a timeout, the
/// previous snapshot (`pixels`) is left untouched and the failure is only
/// logged — a wedged or erroring store never stalls the worker and never
/// empties out a snapshot that was previously known-good.
async fn refresh_pixel_snapshot(store: &Arc<dyn Store>, pixels: &mut Vec<PixelConfig>) {
    match tokio::time::timeout(PIXEL_SNAPSHOT_TIMEOUT, store.list_pixels()).await {
        Ok(Ok(configs)) => *pixels = configs,
        Ok(Err(e)) => {
            eprintln!("{}", serde_json::json!({"pixel_list_error": e.to_string()}));
        }
        Err(_) => {
            eprintln!(
                "{}",
                serde_json::json!({"pixel_list_error": "timed out refreshing pixel snapshot"})
            );
        }
    }
}

async fn flush(
    sink: &Arc<dyn AnalyticsSink>,
    buf: &mut Vec<ClickEvent>,
    pixels: &[PixelConfig],
    client: &reqwest::Client,
    key: u64,
    bases: &PixelBases,
) {
    if buf.is_empty() {
        return;
    }
    if let Err(e) = sink.record_batch(buf).await {
        eprintln!(
            "{}",
            serde_json::json!({"analytics_flush_error": e.to_string()})
        );
    }
    forward_to_pixels(pixels, client, key, bases, buf).await;
    buf.clear();
}

/// Forwards the flushed batch to every active pixel config in the cached
/// `pixels` snapshot (no store access on this path — see `spawn_worker`).
/// Fail-open: a per-provider forward failure is only logged, never
/// propagated (never affects the sink write above nor the redirect hot path,
/// which has already returned by the time this runs).
async fn forward_to_pixels(
    pixels: &[PixelConfig],
    client: &reqwest::Client,
    key: u64,
    bases: &PixelBases,
    events: &[ClickEvent],
) {
    for config in pixels.iter().filter(|c| c.active) {
        let base = bases.base_for(config.provider);
        if let Err(e) = pixel::forward(client, base, config, events, key).await {
            eprintln!(
                "{}",
                serde_json::json!({
                    "pixel_forward_error": e.to_string(),
                    "pixel_id": config.id,
                })
            );
        }
    }
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
            ip: None,
            fbc: None,
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
        let (store, sink) = crate::store::open_backends(dir.path()).await.unwrap();
        let (tx, rx) = tokio::sync::mpsc::channel::<ClickEvent>(1000);
        let handle = spawn_worker(
            rx,
            sink.clone(),
            store,
            reqwest::Client::new(),
            0x1234,
            PixelBases::default(),
        );

        for i in 0..250u64 {
            tx.send(ClickEvent {
                id: 5,
                ts: 1_752_300_000 + i,
                referer: None,
                country: Some("BR".into()),
                user_agent: Some("iPhone".into()),
                ip: None,
                fbc: None,
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
    fn old_clickevent_json_without_ip_fbc_deserializes_with_none() {
        let blob = r#"{"id":1,"ts":2,"referer":null,"country":"BR","user_agent":"UA"}"#;
        let ev: ClickEvent = serde_json::from_str(blob).unwrap();
        assert_eq!(ev.id, 1);
        assert_eq!(ev.country.as_deref(), Some("BR"));
        assert_eq!(ev.ip, None);
        assert_eq!(ev.fbc, None);
    }

    #[test]
    fn serialized_clickevent_never_contains_ip_or_fbc() {
        let ev = ClickEvent {
            id: 7,
            ts: 100,
            referer: None,
            country: Some("BR".into()),
            user_agent: Some("UA".into()),
            ip: Some("203.0.113.9".into()),
            fbc: Some("fb.1.100000.abc123".into()),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(!json.contains("203.0.113.9"));
        assert!(!json.contains("fb.1.100000.abc123"));
        assert!(!json.contains("\"ip\""));
        assert!(!json.contains("\"fbc\""));
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
            ip: None,
            fbc: None,
        });
        a.apply(&ClickEvent {
            id: 1,
            ts: 5_000_000_000,
            referer: None,
            country: None,
            user_agent: None,
            ip: None,
            fbc: None,
        });
        assert_eq!(a.first_ts, 0);
        assert_eq!(a.last_ts, 5_000_000_000);
    }
}
