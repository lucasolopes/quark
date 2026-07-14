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
    pub city: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Aggregates {
    pub total: u64,
    pub first_ts: u64,
    pub last_ts: u64,
    pub per_day: BTreeMap<String, u64>,
    pub per_country: BTreeMap<String, u64>,
    pub per_device: BTreeMap<String, u64>,
    #[serde(default)]
    pub per_os: BTreeMap<String, u64>,
    #[serde(default)]
    pub per_browser: BTreeMap<String, u64>,
    #[serde(default)]
    pub per_referer: BTreeMap<String, u64>,
    #[serde(default)]
    pub per_city: BTreeMap<String, u64>,
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
        let os = os_from_ua(ev.user_agent.as_deref());
        *self.per_os.entry(os.to_string()).or_insert(0) += 1;
        let browser = browser_from_ua(ev.user_agent.as_deref());
        *self.per_browser.entry(browser.to_string()).or_insert(0) += 1;
        let referer = referer_host(ev.referer.as_deref());
        *self.per_referer.entry(referer).or_insert(0) += 1;
        if let Some(city) = &ev.city {
            if !city.is_empty() {
                *self.per_city.entry(city.clone()).or_insert(0) += 1;
            }
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

/// Lightweight OS heuristic from the User-Agent (no external dep).
///
/// Order matters: iPhone/iPad match before Macintosh (both mention Mac-ish
/// tokens on iOS Safari's UA string), and Android matches before Linux
/// (Android UAs also contain "linux").
pub fn os_from_ua(ua: Option<&str>) -> &'static str {
    match ua {
        Some(s) => {
            let s = s.to_ascii_lowercase();
            if s.contains("iphone") || s.contains("ipad") {
                "iOS"
            } else if s.contains("android") {
                "Android"
            } else if s.contains("windows") {
                "Windows"
            } else if s.contains("macintosh") || s.contains("mac os") {
                "macOS"
            } else if s.contains("linux") {
                "Linux"
            } else {
                "Other"
            }
        }
        None => "Other",
    }
}

/// Lightweight browser heuristic from the User-Agent (no external dep).
///
/// Order matters: Edge (Chromium-based) mentions "edg" alongside "chrome",
/// so it must match first; Chrome mentions "safari" too, so it must match
/// before Safari.
pub fn browser_from_ua(ua: Option<&str>) -> &'static str {
    match ua {
        Some(s) => {
            let s = s.to_ascii_lowercase();
            if s.contains("edg/")
                || s.contains("edge/")
                || s.contains("edga/")
                || s.contains("edgios/")
            {
                "Edge"
            } else if s.contains("chrome") || s.contains("crios") {
                "Chrome"
            } else if s.contains("firefox") {
                "Firefox"
            } else if s.contains("safari") {
                "Safari"
            } else {
                "Other"
            }
        }
        None => "Other",
    }
}

/// Groups a referer by hostname (no scheme/port/path), to keep cardinality
/// bounded. Absent or empty referer becomes `"direct"`; an unparseable
/// referer falls back to `"other"`.
pub fn referer_host(referer: Option<&str>) -> String {
    match referer {
        None => "direct".to_string(),
        Some(s) if s.trim().is_empty() => "direct".to_string(),
        Some(s) => match url::Url::parse(s) {
            Ok(u) => match u.host_str() {
                Some(h) => h.to_string(),
                None => "other".to_string(),
            },
            Err(_) => "other".to_string(),
        },
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
            city: None,
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
                city: None,
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
            city: None,
        });
        a.apply(&ClickEvent {
            id: 1,
            ts: 5_000_000_000,
            referer: None,
            country: None,
            user_agent: None,
            city: None,
        });
        assert_eq!(a.first_ts, 0);
        assert_eq!(a.last_ts, 5_000_000_000);
    }

    #[test]
    fn os_heuristic() {
        assert_eq!(
            os_from_ua(Some(
                "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X)"
            )),
            "iOS"
        );
        assert_eq!(
            os_from_ua(Some("Mozilla/5.0 (iPad; CPU OS 17_0 like Mac OS X)")),
            "iOS"
        );
        assert_eq!(
            os_from_ua(Some("Mozilla/5.0 (Linux; Android 14; Pixel 8)")),
            "Android"
        );
        assert_eq!(
            os_from_ua(Some("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")),
            "Windows"
        );
        assert_eq!(
            os_from_ua(Some("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)")),
            "macOS"
        );
        assert_eq!(os_from_ua(Some("Mozilla/5.0 (X11; Linux x86_64)")), "Linux");
        assert_eq!(os_from_ua(Some("curl/8.0")), "Other");
        assert_eq!(os_from_ua(None), "Other");
    }

    #[test]
    fn browser_heuristic() {
        assert_eq!(
            browser_from_ua(Some(
                "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1"
            )),
            "Safari"
        );
        assert_eq!(
            browser_from_ua(Some(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"
            )),
            "Chrome"
        );
        assert_eq!(
            browser_from_ua(Some(
                "Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Mobile Safari/537.36"
            )),
            "Chrome"
        );
        assert_eq!(
            browser_from_ua(Some(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36 Edg/120.0.0.0"
            )),
            "Edge"
        );
        assert_eq!(
            browser_from_ua(Some(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:121.0) Gecko/20100101 Firefox/121.0"
            )),
            "Firefox"
        );
        assert_eq!(browser_from_ua(Some("curl/8.0")), "Other");
        assert_eq!(browser_from_ua(None), "Other");
    }

    #[test]
    fn referer_host_variants() {
        assert_eq!(
            referer_host(Some("https://news.ycombinator.com/x")),
            "news.ycombinator.com"
        );
        assert_eq!(
            referer_host(Some("https://sub.example.com:8443/path?q=1")),
            "sub.example.com"
        );
        assert_eq!(referer_host(None), "direct");
        assert_eq!(referer_host(Some("")), "direct");
        assert_eq!(referer_host(Some("not a url")), "other");
    }

    #[test]
    fn aggregates_populates_os_browser_referer_city() {
        let mut a = Aggregates::default();
        a.apply(&ClickEvent {
            id: 1,
            ts: 1_752_300_000,
            referer: Some("https://news.ycombinator.com/x".into()),
            country: Some("BR".into()),
            user_agent: Some(
                "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1".into(),
            ),
            city: Some("Sao Paulo".into()),
        });
        a.apply(&ClickEvent {
            id: 1,
            ts: 1_752_300_050,
            referer: None,
            country: Some("US".into()),
            user_agent: Some(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36".into(),
            ),
            city: None,
        });
        a.apply(&ClickEvent {
            id: 1,
            ts: 1_752_300_100,
            referer: Some("https://news.ycombinator.com/y".into()),
            country: Some("US".into()),
            user_agent: Some(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:121.0) Gecko/20100101 Firefox/121.0"
                    .into(),
            ),
            city: Some("".into()),
        });

        assert_eq!(a.per_os.get("iOS"), Some(&1));
        assert_eq!(a.per_os.get("Windows"), Some(&2));
        assert_eq!(a.per_browser.get("Safari"), Some(&1));
        assert_eq!(a.per_browser.get("Chrome"), Some(&1));
        assert_eq!(a.per_browser.get("Firefox"), Some(&1));
        assert_eq!(a.per_referer.get("news.ycombinator.com"), Some(&2));
        assert_eq!(a.per_referer.get("direct"), Some(&1));
        assert_eq!(a.per_city.get("Sao Paulo"), Some(&1));
        assert_eq!(a.per_city.len(), 1, "empty city must not pollute per_city");
    }

    #[test]
    fn aggregates_deserializes_pre_branch_blob_without_new_fields() {
        let old_json = r#"{
            "total": 3,
            "first_ts": 1752300000,
            "last_ts": 1752400000,
            "per_day": {"2025-07-12": 3},
            "per_country": {"BR": 2, "US": 1},
            "per_device": {"Mobile": 1, "Desktop": 2}
        }"#;

        let a: Aggregates =
            serde_json::from_str(old_json).expect("old blob without new fields must deserialize");

        assert_eq!(a.total, 3);
        assert_eq!(a.per_country.get("BR"), Some(&2));
        assert!(a.per_os.is_empty());
        assert!(a.per_browser.is_empty());
        assert!(a.per_referer.is_empty());
        assert!(a.per_city.is_empty());
    }
}
