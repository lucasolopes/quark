use crate::pixel::{self, PixelBases, PixelConfig};
use crate::store::{AlertRule, Store, StoreError};
use crate::tenant::TenantId;
use crate::webhooks::delivery::WebhookDispatcher;
use crate::webhooks::{EventType, WebhookEvent};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc::Receiver;

pub mod clickhouse;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClickEvent {
    pub id: u64,
    /// Stable per-click id, generated once at capture in the redirect handler
    /// (`clk_` + 16 random bytes hex). Carried through the in-memory channel to
    /// the worker, so the same value is sent on every retry of this click. Used
    /// as the Meta CAPI `event_id` (real dedup) and the GA4 `transaction_id`
    /// param. `serde(default)` keeps old recent-events blobs deserializing
    /// (empty string); unlike `ip`/`fbc` it DOES persist in the recent buffer,
    /// since a replay-safe id is exactly what idempotent sink writes will reuse.
    #[serde(default)]
    pub event_id: String,
    pub ts: u64,
    pub referer: Option<String>,
    pub country: Option<String>,
    pub user_agent: Option<String>,
    pub city: Option<String>,
    /// Response-side flag: whether `user_agent` looks like a bot/crawler.
    /// Not necessarily persisted with a meaningful value; the `Stats`
    /// builder (re)computes it via `is_bot` for every event in `recent`.
    #[serde(default)]
    pub bot: bool,
    /// Captured only to forward server-side conversions (Meta CAPI user_data).
    /// `serde(skip)` keeps them in memory for the worker but out of the
    /// persisted recent-events buffer, so the raw IP never lands on disk.
    #[serde(skip)]
    pub ip: Option<String>,
    #[serde(skip)]
    pub fbc: Option<String>,
    /// Index of the A/B variant served for this click; `None` when the link
    /// has no variants (the common case).
    #[serde(default)]
    pub variant: Option<u32>,
    /// Owning tenant of the link this click hit, stamped from the
    /// authoritative `Record` at redirect time. `serde(default)` keeps old
    /// persisted/cached blobs (pre multi-tenancy P4a) deserializing as 0,
    /// the default tenant.
    #[serde(default)]
    pub tenant_id: u64,
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
    /// Count of clicks flagged as bot/crawler by `is_bot`. These clicks are
    /// still counted in `total` (an honest raw count) but are excluded from
    /// every `per_*` breakdown, which are human-only.
    #[serde(default)]
    pub bots: u64,
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
        if is_bot(ev.user_agent.as_deref()) {
            self.bots += 1;
            return;
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
        if let Some(variant) = ev.variant {
            *self.per_variant.entry(variant.to_string()).or_insert(0) += 1;
        }
    }

    /// Folds another `Aggregates`'s counts into `self` — combines several
    /// per-link aggregates into one per-tenant aggregate. Used by backends
    /// (LMDB's `stats_for_tenant`) that have no query engine to do the
    /// summing for them; Postgres/ClickHouse do this with `SUM()`/`GROUP BY`
    /// instead and never call this.
    ///
    /// `first_ts`/`last_ts` are timestamps, not counts: an `other` with no
    /// events (`total == 0`) carries meaningless zeros there and is skipped
    /// for those two fields specifically, so an empty aggregate can never
    /// drag `first_ts` down to 0 or leave `last_ts` unmoved incorrectly.
    pub fn merge(&mut self, other: &Aggregates) {
        let self_had_data = self.total > 0;
        self.total += other.total;
        self.bots += other.bots;
        if other.total > 0 {
            if !self_had_data || other.first_ts < self.first_ts {
                self.first_ts = other.first_ts;
            }
            if other.last_ts > self.last_ts {
                self.last_ts = other.last_ts;
            }
        }
        for (k, v) in &other.per_day {
            *self.per_day.entry(k.clone()).or_insert(0) += v;
        }
        for (k, v) in &other.per_country {
            *self.per_country.entry(k.clone()).or_insert(0) += v;
        }
        for (k, v) in &other.per_device {
            *self.per_device.entry(k.clone()).or_insert(0) += v;
        }
        for (k, v) in &other.per_os {
            *self.per_os.entry(k.clone()).or_insert(0) += v;
        }
        for (k, v) in &other.per_browser {
            *self.per_browser.entry(k.clone()).or_insert(0) += v;
        }
        for (k, v) in &other.per_referer {
            *self.per_referer.entry(k.clone()).or_insert(0) += v;
        }
        for (k, v) in &other.per_city {
            *self.per_city.entry(k.clone()).or_insert(0) += v;
        }
        for (k, v) in &other.per_variant {
            *self.per_variant.entry(k.clone()).or_insert(0) += v;
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

/// Lightweight bot/crawler heuristic from the User-Agent (no external dep,
/// same style as `device_from_ua`/`os_from_ua`).
///
/// Case-insensitive substring match against common crawler/bot/library
/// tokens. An empty or absent User-Agent is treated as a bot: no real
/// browser sends no UA. This is a heuristic only ("potential" bots), not a
/// guarantee.
pub fn is_bot(ua: Option<&str>) -> bool {
    const BOT_TOKENS: &[&str] = &[
        "bot",
        "crawler",
        "spider",
        "crawl",
        "slurp",
        "bingpreview",
        "facebookexternalhit",
        "embedly",
        "curl",
        "wget",
        "python-requests",
        "httpie",
        "go-http-client",
        "axios",
        "headless",
        "phantomjs",
        "preview",
        "monitor",
        "uptime",
        "pingdom",
    ];
    match ua {
        Some(s) if !s.is_empty() => {
            let s = s.to_ascii_lowercase();
            BOT_TOKENS.iter().any(|t| s.contains(t))
        }
        _ => true,
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
    /// Aggregate analytics across every link owned by `tenant` — the "all my
    /// links" view behind `GET /admin/stats` (multi-tenancy P4a). Unlike
    /// `stats`, there's no single link to key `recent` off of, so this
    /// returns aggregates only, and it never returns `None`: a tenant with no
    /// clicks yet gets `Aggregates::default()`, not a missing-record signal.
    async fn stats_for_tenant(&self, tenant: u64) -> Result<Aggregates, StoreError>;
}

/// Batch size that triggers an immediate flush (in addition to the 5s timer).
pub const BATCH: usize = 500;

/// How long a full pixel-snapshot refresh (`list_tenants` + `list_pixels` per
/// tenant) is allowed to run before it's abandoned in favor of the previous
/// snapshot (fail-open: a wedged store must never stall the worker).
const PIXEL_SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(3);

/// Per-replica in-memory state for `AlertCounter::Memory`: for each
/// `(tenant, link_id)`, the current fixed window, the click count in it, and
/// whether `link.threshold_reached` has already fired this window. The entry
/// resets when the window rolls over, so the map is bounded by the number of
/// links that carry a rule (not by time).
#[derive(Default)]
struct AlertMemState {
    map: HashMap<(u64, u64), (u64, u32, bool)>,
}

/// Shared fixed-window click counter for threshold alerts (LUC-38). Mirrors
/// `abuse::ratelimit`: Valkey `INCR`+`EXPIRE` keyed by
/// `quark:alert:cnt:{tenant}:{id}:{window}` when a control connection is
/// present, else a per-replica in-memory count. Fire-once-per-window is a
/// `SET NX EX` marker (`quark:alert:fired:...`) on Valkey, or the `fired` flag
/// in the memory entry. Fail-open: a Valkey error only logs and never fires,
/// never panics the worker.
enum AlertCounter {
    Memory(Mutex<AlertMemState>),
    Valkey(redis::aio::MultiplexedConnection),
}

impl AlertCounter {
    fn memory() -> AlertCounter {
        AlertCounter::Memory(Mutex::new(AlertMemState::default()))
    }

    /// `Some(conn)` -> shared Valkey counter; `None` -> per-replica memory.
    fn new(control: Option<redis::aio::MultiplexedConnection>) -> AlertCounter {
        match control {
            Some(conn) => AlertCounter::Valkey(conn),
            None => AlertCounter::memory(),
        }
    }

    /// Records one click against `rule` at time `ts`. Returns `Some(count)`
    /// exactly once per fixed window: on the click that first brings the
    /// window's count to `>= rule.threshold`. Subsequent clicks in the same
    /// window return `None` (already fired); a new window can fire again.
    async fn observe(&self, tenant: u64, id: u64, rule: &AlertRule, ts: u64) -> Option<u64> {
        let window_secs = rule.window_secs.max(1);
        let window = ts / window_secs;
        match self {
            AlertCounter::Memory(state) => {
                let mut st = state.lock().unwrap();
                let entry = st.map.entry((tenant, id)).or_insert((window, 0, false));
                if entry.0 != window {
                    *entry = (window, 0, false);
                }
                entry.1 += 1;
                if entry.1 >= rule.threshold && !entry.2 {
                    entry.2 = true;
                    Some(entry.1 as u64)
                } else {
                    None
                }
            }
            AlertCounter::Valkey(conn) => {
                let mut c = conn.clone();
                let cnt_key = format!("quark:alert:cnt:{tenant}:{id}:{window}");
                let count: i64 = match redis::cmd("INCR").arg(&cnt_key).query_async(&mut c).await {
                    Ok(n) => n,
                    Err(e) => {
                        eprintln!(
                            "{}",
                            serde_json::json!({"alert_counter_error": e.to_string()})
                        );
                        return None;
                    }
                };
                if count == 1 {
                    let _: Result<(), _> = redis::cmd("EXPIRE")
                        .arg(&cnt_key)
                        .arg(window_secs as i64 * 2)
                        .query_async(&mut c)
                        .await;
                }
                if (count as u64) < rule.threshold as u64 {
                    return None;
                }
                // Fire-once marker: SET NX succeeds only for the first crosser
                // of this window across all replicas.
                let fired_key = format!("quark:alert:fired:{tenant}:{id}:{window}");
                let set: Result<Option<String>, _> = redis::cmd("SET")
                    .arg(&fired_key)
                    .arg(1)
                    .arg("NX")
                    .arg("EX")
                    .arg(window_secs as i64 * 2)
                    .query_async(&mut c)
                    .await;
                match set {
                    Ok(Some(_)) => Some(count as u64),
                    Ok(None) => None,
                    Err(e) => {
                        eprintln!(
                            "{}",
                            serde_json::json!({"alert_fire_marker_error": e.to_string()})
                        );
                        None
                    }
                }
            }
        }
    }
}

/// `evt_<32 hex>` event id embedded in an alert payload's `id` field, matching
/// the shape `api::webhook_event_payload` uses for lifecycle events.
fn generate_alert_event_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("system RNG must be available");
    let hex = crate::hex(&bytes);
    format!("evt_{hex}")
}

/// Builds the `link.threshold_reached` JSON body in the same `{id, type,
/// timestamp, data}` envelope as the other webhook events. `data` carries the
/// short `code`, the `count` that crossed the threshold, the configured
/// `threshold`/`window_secs`, and the triggering click `ts`.
fn threshold_payload(code: &str, count: u64, threshold: u32, window_secs: u64, ts: u64) -> String {
    serde_json::json!({
        "id": generate_alert_event_id(),
        "type": EventType::LinkThresholdReached.as_str(),
        "timestamp": crate::now(),
        "data": {
            "code": code,
            "count": count,
            "threshold": threshold,
            "window_secs": window_secs,
            "ts": ts,
        },
    })
    .to_string()
}

/// For every click whose `(tenant, id)` has a rule in `alerts`, feeds the
/// shared window counter and, on the first crossing of the window, emits
/// `link.threshold_reached` through the existing webhook delivery path.
/// Off the redirect hot path (runs in the flush), best-effort, and fail-open
/// (the counter itself swallows Valkey errors).
async fn process_alerts(
    counter: &AlertCounter,
    alerts: &[(TenantId, HashMap<u64, AlertRule>)],
    webhooks: &WebhookDispatcher,
    key: u64,
    events: &[ClickEvent],
) {
    if alerts.is_empty() {
        return;
    }
    for ev in events {
        let Some((_, tenant_rules)) = alerts.iter().find(|(t, _)| t.0 == ev.tenant_id) else {
            continue;
        };
        let Some(rule) = tenant_rules.get(&ev.id) else {
            continue;
        };
        if let Some(count) = counter.observe(ev.tenant_id, ev.id, rule, ev.ts).await {
            let code = crate::codec::to_base62(crate::permute::encode(ev.id, key));
            let body = threshold_payload(&code, count, rule.threshold, rule.window_secs, ev.ts);
            webhooks.emit(WebhookEvent {
                event_type: EventType::LinkThresholdReached,
                body,
                tenant_id: TenantId(ev.tenant_id),
            });
        }
    }
}

/// Refreshes the cached alert-rule snapshot from `store` across every tenant,
/// bounded by `PIXEL_SNAPSHOT_TIMEOUT`, mirroring `refresh_pixel_snapshot`.
/// Fail-open: on a store error or timeout the previous snapshot is left
/// untouched and only a line is logged, so a wedged store never stalls the
/// worker nor empties a known-good snapshot.
async fn refresh_alert_snapshot(
    store: &Arc<dyn Store>,
    alerts: &mut Vec<(TenantId, HashMap<u64, AlertRule>)>,
) {
    let load = async {
        let tenants = store.list_tenants().await?;
        let mut out: Vec<(TenantId, HashMap<u64, AlertRule>)> = Vec::with_capacity(tenants.len());
        for t in tenants {
            let rules = store.list_alert_rules(t.id).await?;
            if !rules.is_empty() {
                out.push((t.id, rules.into_iter().collect()));
            }
        }
        Ok::<_, StoreError>(out)
    };
    match tokio::time::timeout(PIXEL_SNAPSHOT_TIMEOUT, load).await {
        Ok(Ok(snapshot)) => *alerts = snapshot,
        Ok(Err(e)) => {
            eprintln!("{}", serde_json::json!({"alert_list_error": e.to_string()}));
        }
        Err(_) => {
            eprintln!(
                "{}",
                serde_json::json!({"alert_list_error": "timed out refreshing alert snapshot"})
            );
        }
    }
}

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
///
/// `webhooks` is the shared dispatcher used to emit `link.threshold_reached`
/// (LUC-38), the same best-effort in-memory path `clicked`/`expired` use;
/// `control` is the Valkey control connection (`None` -> per-replica in-memory
/// counting). The alert-rule snapshot is refreshed on the same 5s tick as
/// pixels, fail-open.
#[allow(clippy::too_many_arguments)]
pub fn spawn_worker(
    mut rx: Receiver<ClickEvent>,
    sink: Arc<dyn AnalyticsSink>,
    store: Arc<dyn Store>,
    client: reqwest::Client,
    key: u64,
    bases: PixelBases,
    webhooks: Arc<WebhookDispatcher>,
    control: Option<redis::aio::MultiplexedConnection>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut buf: Vec<ClickEvent> = Vec::with_capacity(BATCH);
        // Snapshot de pixels agrupado por tenant. Cada clique é encaminhado só
        // aos pixels do seu próprio tenant (isolamento cross-tenant), então o
        // tenant dono precisa viajar junto: `PixelConfig` não carrega tenant.
        let mut pixels: Vec<(TenantId, Vec<PixelConfig>)> = Vec::new();
        refresh_pixel_snapshot(&store, &mut pixels).await;
        // Snapshot de regras de limiar por tenant (mesmo molde dos pixels).
        let mut alerts: Vec<(TenantId, HashMap<u64, AlertRule>)> = Vec::new();
        refresh_alert_snapshot(&store, &mut alerts).await;
        let counter = AlertCounter::new(control);
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(5));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                maybe = rx.recv() => {
                    match maybe {
                        Some(ev) => {
                            buf.push(ev);
                            if buf.len() >= BATCH {
                                flush(&sink, &mut buf, &pixels, &client, key, &bases, &counter, &alerts, &webhooks).await;
                            }
                        }
                        None => {
                            flush(&sink, &mut buf, &pixels, &client, key, &bases, &counter, &alerts, &webhooks).await;
                            break;
                        }
                    }
                }
                _ = ticker.tick() => {
                    refresh_pixel_snapshot(&store, &mut pixels).await;
                    refresh_alert_snapshot(&store, &mut alerts).await;
                    flush(&sink, &mut buf, &pixels, &client, key, &bases, &counter, &alerts, &webhooks).await;
                }
            }
        }
    })
}

/// Refreshes the cached pixel-config snapshot from `store`, across every
/// tenant, bounded by `PIXEL_SNAPSHOT_TIMEOUT`. Fail-open: on a store error
/// (listing tenants or any tenant's pixels) or a timeout, the previous
/// snapshot (`pixels`) is left untouched and the failure is only logged, so a
/// wedged or erroring store never stalls the worker and never empties out a
/// snapshot that was previously known-good.
///
/// In OSS/single-tenant mode `list_tenants` returns only the default tenant,
/// so this degrades to exactly the old single-tenant behavior.
async fn refresh_pixel_snapshot(
    store: &Arc<dyn Store>,
    pixels: &mut Vec<(TenantId, Vec<PixelConfig>)>,
) {
    let load = async {
        let tenants = store.list_tenants().await?;
        let mut out: Vec<(TenantId, Vec<PixelConfig>)> = Vec::with_capacity(tenants.len());
        for t in tenants {
            let configs = store.list_pixels(t.id).await?;
            if !configs.is_empty() {
                out.push((t.id, configs));
            }
        }
        Ok::<_, StoreError>(out)
    };
    match tokio::time::timeout(PIXEL_SNAPSHOT_TIMEOUT, load).await {
        Ok(Ok(snapshot)) => *pixels = snapshot,
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

#[allow(clippy::too_many_arguments)]
async fn flush(
    sink: &Arc<dyn AnalyticsSink>,
    buf: &mut Vec<ClickEvent>,
    pixels: &[(TenantId, Vec<PixelConfig>)],
    client: &reqwest::Client,
    key: u64,
    bases: &PixelBases,
    counter: &AlertCounter,
    alerts: &[(TenantId, HashMap<u64, AlertRule>)],
    webhooks: &WebhookDispatcher,
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
    process_alerts(counter, alerts, webhooks, key, buf).await;
    buf.clear();
}

/// Forwards the flushed batch to every active pixel config in the cached
/// `pixels` snapshot (no store access on this path, see `spawn_worker`).
///
/// Tenant isolation: the batch mixes clicks from every tenant, so for each
/// tenant we forward only that tenant's own events to that tenant's pixels.
/// A tenant's conversion data never reaches another tenant's provider.
///
/// Fail-open: a per-provider forward failure is only logged, never
/// propagated (never affects the sink write above nor the redirect hot path,
/// which has already returned by the time this runs).
async fn forward_to_pixels(
    pixels: &[(TenantId, Vec<PixelConfig>)],
    client: &reqwest::Client,
    key: u64,
    bases: &PixelBases,
    events: &[ClickEvent],
) {
    for (tenant, configs) in pixels {
        if !configs.iter().any(|c| c.active) {
            continue;
        }
        let scoped: Vec<ClickEvent> = events
            .iter()
            .filter(|e| e.tenant_id == tenant.0)
            .cloned()
            .collect();
        if scoped.is_empty() {
            continue;
        }
        for config in configs.iter().filter(|c| c.active) {
            let base = bases.base_for(config.provider);
            if let Err(e) =
                pixel::forward(client, base, config, &scoped, key, bases.anonymize_ip).await
            {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::webhooks::WebhookEvent;
    use std::sync::atomic::AtomicBool;
    use tokio::sync::mpsc::Receiver as MpscReceiver;

    /// Builds a throwaway `WebhookDispatcher` over a fresh channel, returning
    /// the dispatcher and the receiver so a test can assert on emitted events.
    fn test_dispatcher() -> (Arc<WebhookDispatcher>, MpscReceiver<WebhookEvent>) {
        let (tx, rx) = tokio::sync::mpsc::channel::<WebhookEvent>(64);
        let clicked = Arc::new(AtomicBool::new(false));
        let expired = Arc::new(AtomicBool::new(false));
        (Arc::new(WebhookDispatcher::new(tx, clicked, expired)), rx)
    }

    fn alert_click(id: u64, ts: u64, tenant: u64) -> ClickEvent {
        ClickEvent {
            id,
            event_id: String::new(),
            ts,
            referer: None,
            country: None,
            user_agent: Some("Mozilla/5.0 (iPhone)".into()),
            city: None,
            bot: false,
            ip: None,
            fbc: None,
            variant: None,
            tenant_id: tenant,
        }
    }

    /// N-1 clicks in a window do not fire; the N-th fires exactly one
    /// `link.threshold_reached`; extra clicks in the SAME window do not
    /// re-fire; a new window crosses and fires again.
    #[tokio::test]
    async fn threshold_engine_fires_once_per_window() {
        let counter = AlertCounter::memory();
        let (dispatcher, mut rx) = test_dispatcher();
        let rule = AlertRule {
            threshold: 3,
            window_secs: 60,
        };
        let alerts = vec![(TenantId(0), HashMap::from([(7u64, rule)]))];
        let base = 1_752_300_000u64; // window = base / 60

        // First window: 2 clicks -> nothing.
        let two = vec![alert_click(7, base, 0), alert_click(7, base + 1, 0)];
        process_alerts(&counter, &alerts, &dispatcher, 0x1234, &two).await;
        assert!(rx.try_recv().is_err(), "N-1 clicks must not fire");

        // 3rd click -> exactly one event.
        process_alerts(
            &counter,
            &alerts,
            &dispatcher,
            0x1234,
            &[alert_click(7, base + 2, 0)],
        )
        .await;
        let ev = rx.try_recv().expect("N-th click must fire");
        assert_eq!(ev.event_type, EventType::LinkThresholdReached);
        assert_eq!(ev.tenant_id, TenantId(0));
        assert!(rx.try_recv().is_err(), "only one event on the crossing");

        // More clicks in the SAME window -> no re-fire.
        let more = vec![alert_click(7, base + 3, 0), alert_click(7, base + 4, 0)];
        process_alerts(&counter, &alerts, &dispatcher, 0x1234, &more).await;
        assert!(
            rx.try_recv().is_err(),
            "same-window extras must not re-fire"
        );

        // Next window: 3 fresh clicks -> fires again.
        let next = base + 60;
        let three_next = vec![
            alert_click(7, next, 0),
            alert_click(7, next + 1, 0),
            alert_click(7, next + 2, 0),
        ];
        process_alerts(&counter, &alerts, &dispatcher, 0x1234, &three_next).await;
        let ev2 = rx.try_recv().expect("new window must fire again");
        assert_eq!(ev2.event_type, EventType::LinkThresholdReached);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn threshold_engine_ignores_links_without_a_rule() {
        let counter = AlertCounter::memory();
        let (dispatcher, mut rx) = test_dispatcher();
        let rule = AlertRule {
            threshold: 1,
            window_secs: 60,
        };
        let alerts = vec![(TenantId(0), HashMap::from([(7u64, rule)]))];
        // id 99 has no rule; a single click on it must not fire.
        process_alerts(
            &counter,
            &alerts,
            &dispatcher,
            0x1234,
            &[alert_click(99, 100, 0)],
        )
        .await;
        assert!(rx.try_recv().is_err());
        // id 7 with threshold 1 fires on the first click.
        process_alerts(
            &counter,
            &alerts,
            &dispatcher,
            0x1234,
            &[alert_click(7, 100, 0)],
        )
        .await;
        assert!(rx.try_recv().is_ok());
    }

    #[tokio::test]
    async fn threshold_payload_carries_code_and_counts() {
        let body = threshold_payload("abc123", 5, 5, 60, 1_752_300_000);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["type"], "link.threshold_reached");
        assert_eq!(v["data"]["code"], "abc123");
        assert_eq!(v["data"]["count"], 5);
        assert_eq!(v["data"]["threshold"], 5);
        assert_eq!(v["data"]["window_secs"], 60);
        assert!(v["id"].as_str().unwrap().starts_with("evt_"));
    }

    fn ev(id: u64, ts: u64, country: &str, ua: &str) -> ClickEvent {
        ClickEvent {
            id,
            event_id: String::new(),
            ts,
            referer: None,
            country: Some(country.into()),
            user_agent: Some(ua.into()),
            city: None,
            bot: false,
            ip: None,
            fbc: None,
            variant: None,
            tenant_id: 0,
        }
    }

    #[test]
    fn aggregates_total_day_country_device() {
        let mut a = Aggregates::default();
        a.apply(&ev(1, 1_752_300_000, "BR", "Mozilla/5.0 (iPhone)"));
        a.apply(&ev(1, 1_752_300_050, "BR", "Mozilla/5.0 (Windows NT 10.0)"));
        // Not "curl/8.0": that's a bot UA under `is_bot` and would be
        // excluded from breakdowns (covered separately in
        // `apply_mix_counts_bots_and_excludes_from_breakdowns`).
        a.apply(&ev(1, 1_752_400_000, "US", "SomeOtherClient/1.0"));
        assert_eq!(a.total, 3);
        assert_eq!(a.first_ts, 1_752_300_000);
        assert_eq!(a.last_ts, 1_752_400_000);
        assert_eq!(a.per_country.get("BR"), Some(&2));
        assert_eq!(a.per_country.get("US"), Some(&1));
        assert_eq!(a.per_device.get("Mobile"), Some(&1));
        assert_eq!(a.per_device.get("Desktop"), Some(&1));
        assert_eq!(a.per_device.get("Other"), Some(&1));
        assert_eq!(a.per_day.values().sum::<u64>(), 3);
        assert_eq!(a.bots, 0);
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
        let (store, sink) = crate::store::open_backends(dir.path(), false)
            .await
            .unwrap();
        let (tx, rx) = tokio::sync::mpsc::channel::<ClickEvent>(1000);
        let handle = spawn_worker(
            rx,
            sink.clone(),
            store,
            reqwest::Client::new(),
            0x1234,
            PixelBases::default(),
            test_dispatcher().0,
            None,
        );

        for i in 0..250u64 {
            tx.send(ClickEvent {
                id: 5,
                event_id: String::new(),
                ts: 1_752_300_000 + i,
                referer: None,
                country: Some("BR".into()),
                user_agent: Some("iPhone".into()),
                city: None,
                bot: false,
                ip: None,
                fbc: None,
                variant: None,
                tenant_id: 0,
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
        assert_eq!(
            ev.event_id, "",
            "old blob without `event_id` must default to empty"
        );
    }

    #[test]
    fn serialized_clickevent_never_contains_ip_or_fbc() {
        let ev = ClickEvent {
            id: 7,
            event_id: "clk_persisted".into(),
            ts: 100,
            referer: None,
            country: Some("BR".into()),
            user_agent: Some("UA".into()),
            city: None,
            bot: false,
            ip: Some("203.0.113.9".into()),
            fbc: Some("fb.1.100000.abc123".into()),
            variant: None,
            tenant_id: 0,
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
            event_id: String::new(),
            ts: 0,
            referer: None,
            country: None,
            user_agent: None,
            city: None,
            bot: false,
            ip: None,
            fbc: None,
            variant: None,
            tenant_id: 0,
        });
        a.apply(&ClickEvent {
            id: 1,
            event_id: String::new(),
            ts: 5_000_000_000,
            referer: None,
            country: None,
            user_agent: None,
            city: None,
            bot: false,
            ip: None,
            fbc: None,
            variant: None,
            tenant_id: 0,
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
            event_id: String::new(),
            ts: 1_752_300_000,
            referer: Some("https://news.ycombinator.com/x".into()),
            country: Some("BR".into()),
            user_agent: Some(
                "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1".into(),
            ),
            city: Some("Sao Paulo".into()),
            bot: false,
            ip: None,
            fbc: None,
            variant: None,
            tenant_id: 0,
        });
        a.apply(&ClickEvent {
            id: 1,
            event_id: String::new(),
            ts: 1_752_300_050,
            referer: None,
            country: Some("US".into()),
            user_agent: Some(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36".into(),
            ),
            city: None,
            bot: false,
            ip: None,
            fbc: None,
            variant: None,
            tenant_id: 0,
        });
        a.apply(&ClickEvent {
            id: 1,
            event_id: String::new(),
            ts: 1_752_300_100,
            referer: Some("https://news.ycombinator.com/y".into()),
            country: Some("US".into()),
            user_agent: Some(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:121.0) Gecko/20100101 Firefox/121.0"
                    .into(),
            ),
            city: Some("".into()),
            bot: false,
            ip: None,
            fbc: None,
            variant: None,
            tenant_id: 0,
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
        assert_eq!(a.bots, 0, "old blob without `bots` must default to 0");
    }

    #[test]
    fn click_event_deserializes_pre_branch_blob_without_bot_field() {
        let old_json = r#"{
            "id": 1,
            "ts": 1752300000,
            "referer": null,
            "country": "BR",
            "user_agent": "curl/8.0",
            "city": null
        }"#;

        let e: ClickEvent =
            serde_json::from_str(old_json).expect("old event without `bot` must deserialize");

        assert!(!e.bot, "old blob without `bot` must default to false");
    }

    #[test]
    fn is_bot_googlebot_is_bot() {
        assert!(is_bot(Some(
            "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)"
        )));
    }

    #[test]
    fn is_bot_curl_is_bot() {
        assert!(is_bot(Some("curl/7.68.0")));
    }

    #[test]
    fn is_bot_none_is_bot() {
        assert!(is_bot(None));
    }

    #[test]
    fn is_bot_empty_string_is_bot() {
        assert!(is_bot(Some("")));
    }

    #[test]
    fn is_bot_chrome_desktop_is_not_bot() {
        assert!(!is_bot(Some(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"
        )));
    }

    #[test]
    fn is_bot_iphone_safari_is_not_bot() {
        assert!(!is_bot(Some(
            "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1"
        )));
    }

    #[test]
    fn apply_mix_counts_bots_and_excludes_from_breakdowns() {
        let mut a = Aggregates::default();
        a.apply(&ev(1, 1_752_300_000, "BR", "Mozilla/5.0 (iPhone)"));
        a.apply(&ev(1, 1_752_300_050, "US", "Mozilla/5.0 (Windows NT 10.0)"));
        a.apply(&ev(1, 1_752_300_100, "JP", "Googlebot/2.1"));

        assert_eq!(a.bots, 1);
        assert_eq!(a.total, 3);
        assert!(
            !a.per_country.contains_key("JP"),
            "bot's country must not appear in per_country breakdown"
        );
        assert_eq!(a.per_country.get("BR"), Some(&1));
        assert_eq!(a.per_country.get("US"), Some(&1));
    }

    #[test]
    fn apply_increments_per_variant_only_when_some() {
        let mut a = Aggregates::default();
        a.apply(&ClickEvent {
            id: 1,
            event_id: String::new(),
            ts: 1,
            referer: None,
            country: None,
            user_agent: Some("Mozilla/5.0 (iPhone)".into()),
            city: None,
            bot: false,
            ip: None,
            fbc: None,
            variant: Some(0),
            tenant_id: 0,
        });
        a.apply(&ClickEvent {
            id: 1,
            event_id: String::new(),
            ts: 2,
            referer: None,
            country: None,
            user_agent: Some("Mozilla/5.0 (iPhone)".into()),
            city: None,
            bot: false,
            ip: None,
            fbc: None,
            variant: Some(0),
            tenant_id: 0,
        });
        a.apply(&ClickEvent {
            id: 1,
            event_id: String::new(),
            ts: 3,
            referer: None,
            country: None,
            user_agent: Some("Mozilla/5.0 (iPhone)".into()),
            city: None,
            bot: false,
            ip: None,
            fbc: None,
            variant: Some(1),
            tenant_id: 0,
        });
        a.apply(&ClickEvent {
            id: 1,
            event_id: String::new(),
            ts: 4,
            referer: None,
            country: None,
            user_agent: Some("Mozilla/5.0 (iPhone)".into()),
            city: None,
            bot: false,
            ip: None,
            fbc: None,
            variant: None,
            tenant_id: 0,
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

    #[test]
    fn merge_sums_counts_and_widens_ts_range() {
        let mut a = Aggregates::default();
        a.apply(&ev(1, 100, "BR", "Mozilla/5.0 (iPhone)"));
        a.apply(&ev(1, 200, "BR", "Mozilla/5.0 (Windows NT 10.0)"));

        let mut b = Aggregates::default();
        b.apply(&ev(2, 50, "US", "Mozilla/5.0 (iPhone)"));
        b.apply(&ev(2, 300, "JP", "Googlebot/2.1"));

        a.merge(&b);
        assert_eq!(a.total, 4);
        assert_eq!(a.bots, 1, "b's bot click must carry over");
        assert_eq!(a.first_ts, 50, "widened down to b's earlier first_ts");
        assert_eq!(a.last_ts, 300, "widened up to b's later last_ts");
        assert_eq!(a.per_country.get("BR"), Some(&2));
        assert_eq!(a.per_country.get("US"), Some(&1));
        assert!(
            !a.per_country.contains_key("JP"),
            "bot's country must stay excluded from the breakdown after merge"
        );
    }

    #[test]
    fn merge_empty_other_leaves_self_unchanged() {
        let mut a = Aggregates::default();
        a.apply(&ev(1, 100, "BR", "Mozilla/5.0 (iPhone)"));
        let before = a.clone();

        a.merge(&Aggregates::default());
        assert_eq!(a.total, before.total);
        assert_eq!(a.first_ts, before.first_ts);
        assert_eq!(a.last_ts, before.last_ts);
    }

    #[test]
    fn merge_into_empty_self_adopts_other_ts_range() {
        let mut a = Aggregates::default();
        let mut b = Aggregates::default();
        b.apply(&ev(1, 900, "BR", "Mozilla/5.0 (iPhone)"));

        a.merge(&b);
        assert_eq!(a.total, 1);
        assert_eq!(a.first_ts, 900);
        assert_eq!(a.last_ts, 900);
    }
}
