//! Broken-link monitoring: a background checker that probes each link's
//! destination, records its health, and emits `link.broken`/`link.recovered`
//! webhook events on a healthy<->broken transition.
//!
//! Opt-in (only spawned when `QUARK_HEALTH_CHECK_SECS` is set) and, in a
//! multi-node deployment, run only on the designated node (see `main.rs`). The
//! redirect hot path is never touched by this module.

use crate::abuse::{extract_host, is_internal_host};
use crate::store::{LinkHealth, Record, Store};
use crate::webhooks::delivery::WebhookDispatcher;
use crate::webhooks::{EventType, WebhookEvent};
use crate::{codec, permute};
use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

/// Smallest sweep interval we honor; a smaller `QUARK_HEALTH_CHECK_SECS` is
/// clamped up to this so a misconfiguration cannot hammer destinations.
pub const MIN_CHECK_SECS: u64 = 60;
/// Per-probe timeout.
const PROBE_TIMEOUT_SECS: u64 = 10;
/// Links fetched per `list_links` page during a sweep.
const LIST_PAGE: usize = 500;
/// Destinations probed concurrently within a chunk. Bounds how long a chunk
/// takes (~one probe timeout) so the lease can be renewed between chunks well
/// inside its TTL, even when many destinations time out.
const PROBE_CONCURRENCY: usize = 16;

/// Resultado da ultima entrega/forward de uma integracao. `Never` = conectado
/// mas sem entrega ainda; `Ok` = ultima entrega teve sucesso; `Error` carrega
/// um motivo curto (nunca um segredo/token).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", content = "detail", rename_all = "lowercase")]
pub enum HealthStatus {
    #[default]
    Never,
    Ok,
    Error(String),
}

#[cfg(test)]
mod health_status_tests {
    use super::*;

    #[test]
    fn default_is_never() {
        assert_eq!(HealthStatus::default(), HealthStatus::Never);
    }

    #[test]
    fn serializes_with_state_tag_and_detail_content() {
        assert_eq!(
            serde_json::to_string(&HealthStatus::Never).unwrap(),
            r#"{"state":"never"}"#
        );
        assert_eq!(
            serde_json::to_string(&HealthStatus::Ok).unwrap(),
            r#"{"state":"ok"}"#
        );
        assert_eq!(
            serde_json::to_string(&HealthStatus::Error("boom".into())).unwrap(),
            r#"{"state":"error","detail":"boom"}"#
        );
    }

    #[test]
    fn round_trips() {
        for s in [
            HealthStatus::Never,
            HealthStatus::Ok,
            HealthStatus::Error("timeout".into()),
        ] {
            let j = serde_json::to_string(&s).unwrap();
            let back: HealthStatus = serde_json::from_str(&j).unwrap();
            assert_eq!(s, back);
        }
    }
}

/// Whether an observed HTTP status counts as healthy: `2xx`/`3xx` (a live server,
/// even one that redirects) is healthy; everything else (and no status at all,
/// i.e. a connection error or timeout) is broken.
pub fn classify(status: Option<u16>) -> bool {
    matches!(status, Some(s) if (200..400).contains(&s))
}

/// The reqwest client the checker uses: bounded timeout, no redirect following
/// (a `3xx` is treated as alive, and not following avoids SSRF via redirect).
pub fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(PROBE_TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("reqwest client builds")
}

/// Probes one URL with a `GET` (the response body is never read, so this costs
/// about the same as a HEAD while avoiding the many servers/CDNs that reject or
/// misreport HEAD). Classifies the resulting status.
pub async fn probe(client: &reqwest::Client, url: &str, now: u64) -> LinkHealth {
    let status = client
        .get(url)
        .send()
        .await
        .ok()
        .map(|r| r.status().as_u16());
    LinkHealth {
        checked_at: now,
        status,
        healthy: classify(status),
    }
}

/// Whether a host is safe to probe: it must not be an internal hostname and it
/// must not resolve to any internal/loopback/link-local IP. The DNS resolution
/// closes the SSRF gap that a hostname-only check leaves open (a public name
/// pointing at `169.254.169.254` or an RFC1918 address).
pub async fn safe_to_probe(url: &str) -> bool {
    let Some(host) = extract_host(url) else {
        return false;
    };
    if is_internal_host(&host) {
        return false;
    }
    // Resolve and reject if ANY resolved address is internal. Port is
    // irrelevant to the address classification; use 80 for the lookup. The
    // lookup is bounded by a timeout so a hanging resolver cannot stall a whole
    // probe chunk past the lease TTL.
    let resolved = tokio::time::timeout(
        Duration::from_secs(PROBE_TIMEOUT_SECS),
        tokio::net::lookup_host((host.as_str(), 80u16)),
    )
    .await;
    let Ok(Ok(addrs)) = resolved else {
        return false;
    };
    let mut any = false;
    for a in addrs {
        any = true;
        if is_internal_ip(&a.ip()) {
            return false;
        }
    }
    // No addresses resolved -> cannot verify -> do not probe.
    any
}

/// Whether an IP address is in a range the server must never be pointed at by a
/// stored destination (loopback, private, link-local, unspecified, and IPv6
/// unique-local `fc00::/7`).
fn is_internal_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
        }
        std::net::IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() {
                return true;
            }
            // An IPv4-mapped (`::ffff:a.b.c.d`) or deprecated IPv4-compatible
            // (`::a.b.c.d`) address routes to the embedded v4 target on a
            // dual-stack host, so classify it by that v4 address.
            let seg = v6.segments();
            if seg[..6].iter().all(|&x| x == 0) || v6.to_ipv4_mapped().is_some() {
                let v4 = std::net::Ipv4Addr::new(
                    (seg[6] >> 8) as u8,
                    (seg[6] & 0xff) as u8,
                    (seg[7] >> 8) as u8,
                    (seg[7] & 0xff) as u8,
                );
                return is_internal_ip(&std::net::IpAddr::V4(v4));
            }
            // unique-local fc00::/7 or link-local fe80::/10
            (seg[0] & 0xfe00) == 0xfc00 || (seg[0] & 0xffc0) == 0xfe80
        }
    }
}

/// Builds the webhook event body for a health transition, matching the envelope
/// the lifecycle events use (`{id, type, timestamp, data:{code, url, status}}`).
fn transition_body(event_type: EventType, code: &str, url: &str, status: Option<u16>) -> String {
    let mut data = serde_json::Map::new();
    data.insert(
        "code".to_string(),
        serde_json::Value::String(code.to_string()),
    );
    data.insert(
        "url".to_string(),
        serde_json::Value::String(url.to_string()),
    );
    if let Some(s) = status {
        data.insert("status".to_string(), serde_json::Value::from(s));
    }
    let mut id = [0u8; 16];
    let _ = getrandom::fill(&mut id);
    let hex = crate::hex(&id);
    serde_json::json!({
        "id": format!("evt_{hex}"),
        "type": event_type.as_str(),
        "timestamp": crate::now(),
        "data": serde_json::Value::Object(data),
    })
    .to_string()
}

/// Runs one sweep: probe every link (skipping internal hosts), record its
/// health, and emit a transition event when a link flips healthy<->broken.
/// Generic over `prober` so tests can drive it without real HTTP; production
/// passes a closure over [`probe`]. Returns the number of links probed.
///
/// A link never seen before is treated as previously healthy, so a
/// newly-discovered broken destination fires `link.broken` exactly once (the
/// health it writes suppresses a repeat on the next sweep).
/// Returns `(probed_count, lost_lease)`; `lost_lease` is true when the sweep
/// stopped early because another node took over the lease.
pub async fn sweep<P, F, R, RF>(
    store: &Arc<dyn Store>,
    webhooks: &WebhookDispatcher,
    key: u64,
    renew: R,
    prober: P,
) -> Result<(usize, bool), String>
where
    P: Fn(String) -> F,
    F: Future<Output = Option<LinkHealth>>,
    R: Fn() -> RF,
    RF: Future<Output = bool>,
{
    let prev: HashMap<u64, LinkHealth> = store
        .list_link_health(crate::tenant::DEFAULT_TENANT)
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .collect();
    let mut after: Option<u64> = None;
    let mut checked = 0usize;
    // The caller acquired the lease immediately before this call, so the first
    // chunk needs no renewal; subsequent chunks renew to hold it across a long sweep.
    let mut first_chunk = true;
    loop {
        let page = store
            .list_links(
                crate::tenant::DEFAULT_TENANT,
                after,
                LIST_PAGE,
                None,
                None,
                false,
            )
            .await
            .map_err(|e| e.to_string())?;
        let n = page.len();
        if n == 0 {
            break;
        }
        after = page.last().map(|(id, _)| *id);
        // Probe in bounded-concurrency chunks; renew the lease before each chunk
        // (each chunk lasts about one probe timeout, far inside the lease TTL) so
        // a slow sweep keeps the lease and no second node starts a rival sweep.
        let mut page = page;
        while !page.is_empty() {
            let take = page.len().min(PROBE_CONCURRENCY);
            let chunk: Vec<(u64, Record)> = page.drain(..take).collect();
            if !first_chunk && !renew().await {
                // Lost the lease (another node took over): stop this sweep.
                return Ok((checked, true));
            }
            first_chunk = false;
            let results: Vec<(u64, String, Option<LinkHealth>)> = stream::iter(chunk)
                .map(|(id, rec)| {
                    let prober = &prober;
                    async move {
                        let health = prober(rec.url.clone()).await;
                        (id, rec.url, health)
                    }
                })
                .buffer_unordered(PROBE_CONCURRENCY)
                .collect()
                .await;
            for (id, url, maybe_health) in results {
                // None => skipped (internal/unresolvable), left unchecked.
                let Some(health) = maybe_health else {
                    continue;
                };
                checked += 1;
                let was_healthy = prev.get(&id).is_none_or(|p| p.healthy);
                if was_healthy != health.healthy {
                    let et = if health.healthy {
                        EventType::LinkRecovered
                    } else {
                        EventType::LinkBroken
                    };
                    let code = codec::to_base62(permute::encode(id, key));
                    // Persist the new state only once the transition event is
                    // enqueued; if the best-effort channel is full, leave the old
                    // state so the next sweep retries (no permanently-lost alert).
                    // The sweep itself is scoped to `DEFAULT_TENANT` (both
                    // `list_link_health` and `list_links` above are called
                    // with it), so every link seen here already belongs to
                    // `DEFAULT_TENANT` (no tenant travels with `rec` this
                    // far into the loop), but hardcoding it is equivalent to
                    // threading `rec.tenant_id` through.
                    if !webhooks.try_emit(WebhookEvent {
                        event_type: et,
                        body: transition_body(et, &code, &url, health.status),
                        tenant_id: crate::tenant::DEFAULT_TENANT,
                    }) {
                        continue;
                    }
                }
                store
                    .put_link_health(crate::tenant::DEFAULT_TENANT, id, &health)
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
        if n < LIST_PAGE {
            break;
        }
    }
    Ok((checked, false))
}

/// Spawns the periodic checker. The first sweep runs at the first tick
/// (immediately), then every `period`.
pub fn spawn_link_checker(
    store: Arc<dyn Store>,
    webhooks: Arc<WebhookDispatcher>,
    client: reqwest::Client,
    period: Duration,
    key: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Per-process holder id for the sweep lease; any instance may hold it,
        // and only the holder sweeps in a given round (multi-node coordination).
        let mut hb = [0u8; 8];
        let _ = getrandom::fill(&mut hb);
        let holder: String = format!("chk_{}", crate::hex(&hb));
        // Lease lasts longer than one interval so the holder keeps it across a
        // slow sweep; if the holder dies, another node takes over within the TTL.
        let ttl = period.as_secs().saturating_mul(2).max(MIN_CHECK_SECS);
        let mut ticker = tokio::time::interval(period);
        loop {
            ticker.tick().await;
            match store.try_acquire_health_lease(&holder, ttl).await {
                Ok(true) => {}
                // Another instance holds the lease this round.
                Ok(false) => continue,
                Err(e) => {
                    eprintln!(
                        "{}",
                        serde_json::json!({ "health_lease_error": e.to_string() })
                    );
                    continue;
                }
            }
            let now = crate::now();
            let client = &client;
            let prober = move |url: String| {
                let client = client.clone();
                async move {
                    if safe_to_probe(&url).await {
                        Some(probe(&client, &url, now).await)
                    } else {
                        None
                    }
                }
            };
            // Renew the lease between chunks so a slow sweep keeps it. On a
            // transient store error assume we still hold it (unwrap_or(true)) so
            // a DB blip does not abort the sweep; only a definitive Ok(false)
            // (another node took over) stops it.
            let renew = || async {
                store
                    .try_acquire_health_lease(&holder, ttl)
                    .await
                    .unwrap_or(true)
            };
            match sweep(&store, &webhooks, key, renew, prober).await {
                Ok((n, false)) => eprintln!("{}", serde_json::json!({ "health_sweep_checked": n })),
                Ok((n, true)) => {
                    eprintln!("{}", serde_json::json!({ "health_sweep_lease_lost": n }))
                }
                Err(e) => eprintln!("{}", serde_json::json!({ "health_sweep_error": e })),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::lmdb::LmdbStore;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn classify_healthy_and_broken() {
        assert!(classify(Some(200)));
        assert!(classify(Some(301)));
        assert!(classify(Some(399)));
        assert!(!classify(Some(400)));
        assert!(!classify(Some(404)));
        assert!(!classify(Some(500)));
        assert!(!classify(None)); // connection error / timeout
    }

    #[test]
    fn internal_ip_classification() {
        use std::net::IpAddr;
        for s in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.0.1",
            "169.254.169.254", // cloud metadata (link-local)
            "0.0.0.0",
            "::1",
            "fc00::1",                // unique-local
            "fe80::1",                // link-local
            "::ffff:127.0.0.1",       // IPv4-mapped loopback
            "::ffff:169.254.169.254", // IPv4-mapped metadata
            "::7f00:1",               // IPv4-compatible 127.0.0.1 (deprecated)
            "::a9fe:a9fe",            // IPv4-compatible 169.254.169.254
        ] {
            assert!(
                super::is_internal_ip(&s.parse::<IpAddr>().unwrap()),
                "{s} must be internal"
            );
        }
        for s in ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(
                !super::is_internal_ip(&s.parse::<IpAddr>().unwrap()),
                "{s} must be public"
            );
        }
    }

    #[tokio::test]
    async fn safe_to_probe_rejects_internal_and_bad_urls() {
        assert!(!safe_to_probe("http://127.0.0.1/x").await); // internal literal
        assert!(!safe_to_probe("not a url").await); // no host
        assert!(!safe_to_probe("http://this-host-should-not-resolve.invalid/").await);
        // unresolvable
    }

    fn rec(url: &str) -> Record {
        Record {
            url: url.into(),
            expiry: None,
            created: 0,
            tags: Vec::new(),
            max_visits: None,
            rules: Vec::new(),
            variants: Vec::new(),
            app_ios: None,
            app_android: None,
            folder: None,
            fallback_url: None,
            password_hash: None,
            tenant_id: crate::tenant::DEFAULT_TENANT,
        }
    }

    #[tokio::test]
    async fn sweep_records_health_emits_transitions_and_skips_internal() {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn Store> =
            Arc::new(LmdbStore::open_with_node_id(dir.path(), None).unwrap());

        // id 1: external, will probe BROKEN (no prior health -> assumed healthy -> transition).
        store
            .put_link(crate::tenant::DEFAULT_TENANT, 1, &rec("http://a.example/"))
            .await
            .unwrap();
        // id 2: external, was BROKEN, will probe HEALTHY -> recovered.
        store
            .put_link(crate::tenant::DEFAULT_TENANT, 2, &rec("http://b.example/"))
            .await
            .unwrap();
        store
            .put_link_health(
                crate::tenant::DEFAULT_TENANT,
                2,
                &LinkHealth {
                    checked_at: 1,
                    status: Some(500),
                    healthy: false,
                },
            )
            .await
            .unwrap();
        // id 3: internal host, must be SKIPPED (never probed, no health written).
        store
            .put_link(crate::tenant::DEFAULT_TENANT, 3, &rec("http://127.0.0.1/x"))
            .await
            .unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let dispatcher = WebhookDispatcher::new(
            tx,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        );

        // Canned prober: skip internal hosts (None), a.example broken,
        // b.example healthy. Mirrors how the production prober returns None for
        // an unsafe destination.
        let prober = |url: String| async move {
            let host = extract_host(&url).unwrap_or_default();
            if is_internal_host(&host) {
                return None;
            }
            let healthy = url.contains("b.example");
            Some(LinkHealth {
                checked_at: 999,
                status: Some(if healthy { 200 } else { 404 }),
                healthy,
            })
        };
        let renew = || async { true };
        let (checked, lost) = sweep(&store, &dispatcher, 0x1234, renew, prober)
            .await
            .unwrap();
        assert_eq!(checked, 2, "internal link 3 is skipped");
        assert!(!lost, "lease was never lost");

        // Health persisted for 1 and 2 only.
        let health: HashMap<u64, LinkHealth> = store
            .list_link_health(crate::tenant::DEFAULT_TENANT)
            .await
            .unwrap()
            .into_iter()
            .collect();
        assert_eq!(health.len(), 2);
        assert!(!health[&1].healthy);
        assert!(health[&2].healthy);
        assert!(!health.contains_key(&3));

        // Exactly two transition events: one broken, one recovered.
        let mut kinds = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            kinds.push(ev.event_type);
        }
        assert_eq!(kinds.len(), 2);
        assert!(kinds.contains(&EventType::LinkBroken));
        assert!(kinds.contains(&EventType::LinkRecovered));
    }
}
