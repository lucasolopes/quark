//! Outbound webhook delivery: a bounded channel feeds a background worker
//! that snapshots active subscriptions, matches events, guards against SSRF,
//! signs per Standard Webhooks, and POSTs with retry. Delivery is
//! best-effort and fail-open: a full channel or an exhausted retry budget
//! only drops the event and logs a line, it never blocks or panics the
//! caller (in particular the redirect hot path).

use crate::abuse::{extract_host, is_internal_host};
use crate::store::{OutboxDelivery, OutboxRow, Store, StoreError};
use crate::webhooks::{
    channel_payload, format_message, matches, sign, EventType, SubscriptionKind, WebhookEvent,
    WebhookSubscription,
};
use reqwest::redirect::Policy;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::task::JoinHandle;

/// Bound of the in-memory event channel feeding the delivery worker.
pub const WEBHOOK_CHANNEL_CAPACITY: usize = 1024;
/// Number of POST attempts per (subscription, event) before giving up.
pub const DELIVERY_ATTEMPTS: u32 = 3;
/// Per-request timeout for outbound webhook POSTs.
pub const DELIVERY_TIMEOUT_SECS: u64 = 5;

/// How often the worker refreshes its subscription snapshot and the
/// `clicked`/`expired` gating atomics off the ticker branch.
const REFRESH_INTERVAL_SECS: u64 = 10;
/// Base of the exponential backoff between retries (`base * 2^attempt`).
const BACKOFF_BASE_MS: u64 = 200;

/// Front door for emitting webhook events: cheap, non-blocking, fail-open.
///
/// `outbox` is `Some` only on the Postgres backend (wired via `with_outbox`),
/// where lifecycle events (created/updated/deleted) are routed through the
/// durable outbox: the api.rs sites call `lifecycle_deliveries` to build the
/// rows and enqueue them inside the same transaction as the link mutation (a
/// `_tx` store method). On LMDB `outbox` is `None`, `lifecycle_deliveries`
/// returns empty, and `emit_if_in_memory` puts the event on the in-memory
/// channel after the mutation succeeds. `emit` (the in-memory path) is still
/// used for `link.clicked` (hot path) and `link.expired` (also emitted on the
/// redirect hot path, so it must stay off any synchronous DB write).
pub struct WebhookDispatcher {
    tx: Sender<WebhookEvent>,
    pub clicked_subscribed: Arc<AtomicBool>,
    pub expired_subscribed: Arc<AtomicBool>,
    outbox: Option<Arc<dyn Store>>,
}

impl WebhookDispatcher {
    /// Builds a dispatcher over an existing channel sender and the pair of
    /// atomics the worker keeps refreshed (see `spawn_webhook_worker`). The
    /// durable outbox is off by default (`lifecycle_deliveries` returns empty
    /// and `emit_if_in_memory` uses the channel); call `with_outbox` on the
    /// Postgres backend.
    pub fn new(
        tx: Sender<WebhookEvent>,
        clicked_subscribed: Arc<AtomicBool>,
        expired_subscribed: Arc<AtomicBool>,
    ) -> Self {
        Self {
            tx,
            clicked_subscribed,
            expired_subscribed,
            outbox: None,
        }
    }

    /// Enables durable lifecycle routing through the Postgres outbox. Only
    /// `main.rs` calls this, and only on the Postgres backend.
    pub fn with_outbox(mut self, store: Arc<dyn Store>) -> Self {
        self.outbox = Some(store);
        self
    }

    /// Enqueues `ev` for async delivery. Non-blocking: if the worker is
    /// backed up and the channel is full (or closed), the event is dropped
    /// and a line is logged. Never applies backpressure to the caller.
    pub fn emit(&self, ev: WebhookEvent) {
        let _ = self.try_emit(ev);
    }

    /// Like [`emit`] but reports whether the event was enqueued (`true`) or
    /// dropped because the best-effort channel was full (`false`). The link
    /// health checker uses the result to avoid recording a transition it could
    /// not enqueue, so a dropped `link.broken`/`link.recovered` is retried on
    /// the next sweep instead of being lost to one-shot suppression.
    pub fn try_emit(&self, ev: WebhookEvent) -> bool {
        match self.tx.try_send(ev) {
            Ok(()) => true,
            Err(e) => {
                eprintln!(
                    "{}",
                    serde_json::json!({"webhook_event_dropped": e.to_string()})
                );
                false
            }
        }
    }

    /// Emits a lifecycle event on the in-memory channel ONLY when there is no
    /// durable outbox (the LMDB single-node backend). On Postgres the delivery
    /// rows were already enqueued inside the mutation transaction, so this is a
    /// no-op. Callers invoke it only AFTER the mutation succeeds, so a failed
    /// mutation (for example an alias already in use) emits nothing.
    pub fn emit_if_in_memory(&self, ev: WebhookEvent) {
        if self.outbox.is_none() {
            self.emit(ev);
        }
    }

    /// Builds the durable delivery rows for a lifecycle event
    /// (created/updated/deleted) WITHOUT enqueuing them. On the Postgres
    /// backend (`outbox` set) it reads `tenant`'s active subscriptions
    /// (`list_webhooks(tenant)` + `matches`) and returns one `OutboxRow` per
    /// match (`delivery_key = "<event_id>.<sub_id>"`, payload = `ev.body`,
    /// `tenant_id = tenant`); the caller then enqueues those rows inside the
    /// SAME transaction as the link mutation (via the `_tx` store methods),
    /// closing the dual-write window. `tenant` is the link/event's tenant, so
    /// the relay can later resolve the subscription in the right tenant
    /// instead of assuming `DEFAULT_TENANT`. On LMDB (no outbox) it falls back
    /// to the in-memory best-effort `emit` and returns an empty `Vec`
    /// (single-node stays in-memory, unchanged).
    ///
    /// The subscription read is a read, not part of the atomic write, so it
    /// stays outside the mutation's transaction. A store error is logged and
    /// swallowed (returns an empty `Vec`): lifecycle delivery is best-effort at
    /// the admin layer and never fails the admin request. This must NOT be
    /// called from the redirect hot path; `link.clicked`/`link.expired` use
    /// `emit` instead.
    pub async fn lifecycle_deliveries(
        &self,
        tenant: crate::tenant::TenantId,
        ev: &WebhookEvent,
    ) -> Vec<OutboxRow> {
        let Some(store) = &self.outbox else {
            return Vec::new();
        };
        let subs = match store.list_webhooks(tenant).await {
            Ok(subs) => subs,
            Err(e) => {
                eprintln!(
                    "{}",
                    serde_json::json!({"webhook_outbox_snapshot_error": e.to_string()})
                );
                return Vec::new();
            }
        };
        let event_id = outbox_event_id(&ev.body);
        let now = crate::now();
        subs.iter()
            .filter(|s| matches(s, &ev.event_type))
            .map(|s| OutboxRow {
                delivery_key: format!("{event_id}.{}", s.id),
                subscription_id: s.id,
                event_type: ev.event_type.as_str().to_string(),
                payload: ev.body.clone(),
                created: now,
                next_attempt_at: now,
                tenant_id: tenant,
            })
            .collect()
    }
}

/// Extracts the event id from a built webhook payload (the `id` field set by
/// `api::webhook_event_payload`, e.g. `evt_<hex>`), for use as the stable
/// `delivery_key` prefix. Falls back to a fresh random id if the body has no
/// parseable `id`, so a malformed payload can never collapse two distinct
/// events onto one `delivery_key` (which `ON CONFLICT DO NOTHING` would drop).
fn outbox_event_id(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("id").and_then(|id| id.as_str()).map(str::to_string))
        .unwrap_or_else(generate_msg_id)
}

/// Background worker: mirrors `analytics::spawn_worker`'s `tokio::select!`
/// shape. On each event it delivers to the cached subscription snapshot
/// (grouped by tenant, LUC-63); on the ~10s ticker it refreshes that
/// snapshot and the `clicked`/`expired` gating atomics from the store.
pub fn spawn_webhook_worker(
    mut rx: Receiver<WebhookEvent>,
    store: Arc<dyn Store>,
    clicked: Arc<AtomicBool>,
    expired: Arc<AtomicBool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(DELIVERY_TIMEOUT_SECS))
            .redirect(Policy::none())
            .build()
            .expect("reqwest client must build");

        let mut subs: Vec<(crate::tenant::TenantId, Vec<WebhookSubscription>)> = Vec::new();
        refresh_snapshot(&store, &clicked, &expired, &mut subs).await;
        let mut ticker = tokio::time::interval(Duration::from_secs(REFRESH_INTERVAL_SECS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                maybe = rx.recv() => {
                    match maybe {
                        Some(ev) => deliver_to_matching(&client, &subs, &ev).await,
                        None => break,
                    }
                }
                _ = ticker.tick() => {
                    refresh_snapshot(&store, &clicked, &expired, &mut subs).await;
                }
            }
        }
    })
}

/// How long a full subscription-snapshot refresh (`list_tenants` +
/// `list_webhooks` per tenant) is allowed to run before it's abandoned in
/// favor of the previous snapshot (fail-open: a wedged store must never
/// stall the worker, matching the fail-open contract described below).
/// Mirrors `analytics::PIXEL_SNAPSHOT_TIMEOUT`.
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(3);

/// Re-reads subscriptions from the store across every tenant (LUC-63:
/// `list_tenants` + `list_webhooks(t)` per tenant, mirroring
/// `analytics::refresh_pixel_snapshot`), updates the `clicked`/`expired`
/// atomics (true iff ANY tenant has an active subscription for that event
/// type), and writes the fresh per-tenant snapshot into `subs`. Fail-open: on
/// a store error (listing tenants or any tenant's subscriptions) or a
/// timeout, `*subs` is left untouched and the atomics are not touched either:
/// a wedged or erroring store never stalls the worker and never empties out
/// (or falsely degates) a snapshot that was previously known-good.
///
/// In OSS/single-tenant mode `list_tenants` returns only the default
/// tenant, so this degrades to exactly the old single-tenant behavior (one
/// group, same subs).
async fn refresh_snapshot(
    store: &Arc<dyn Store>,
    clicked: &AtomicBool,
    expired: &AtomicBool,
    subs: &mut Vec<(crate::tenant::TenantId, Vec<WebhookSubscription>)>,
) {
    let load = async {
        let tenants = store.list_tenants().await?;
        let mut out = Vec::with_capacity(tenants.len());
        for t in tenants {
            let s = store.list_webhooks(t.id).await?;
            out.push((t.id, s));
        }
        Ok::<_, StoreError>(out)
    };
    match tokio::time::timeout(SNAPSHOT_TIMEOUT, load).await {
        Ok(Ok(snapshot)) => {
            let has_clicked = snapshot.iter().any(|(_, subs)| {
                subs.iter()
                    .any(|s| s.active && s.events.contains(&EventType::LinkClicked))
            });
            let has_expired = snapshot.iter().any(|(_, subs)| {
                subs.iter()
                    .any(|s| s.active && s.events.contains(&EventType::LinkExpired))
            });
            clicked.store(has_clicked, Ordering::Relaxed);
            expired.store(has_expired, Ordering::Relaxed);
            *subs = snapshot;
        }
        Ok(Err(e)) => {
            eprintln!(
                "{}",
                serde_json::json!({"webhook_snapshot_refresh_error": e.to_string()})
            );
        }
        Err(_) => {
            eprintln!(
                "{}",
                serde_json::json!({"webhook_snapshot_refresh_error": "timed out refreshing subscription snapshot"})
            );
        }
    }
}

/// Delivers `ev` to every subscription in `ev.tenant_id`'s group that
/// matches it, skipping internal destinations (SSRF guard via
/// `abuse::is_internal_host`). An event never reaches another tenant's
/// subscriptions (cross-tenant isolation, LUC-63): only the group whose key
/// equals `ev.tenant_id` is consulted.
async fn deliver_to_matching(
    client: &reqwest::Client,
    subs: &[(crate::tenant::TenantId, Vec<WebhookSubscription>)],
    ev: &WebhookEvent,
) {
    deliver_to_matching_guarded(client, subs, ev, is_internal_host).await
}

/// Same as `deliver_to_matching`, but with the SSRF host-block predicate
/// injected. Production always calls `deliver_to_matching`, which wires in
/// the real `is_internal_host`; tests that need to exercise real HTTP
/// delivery (signing, headers, retry) against a local test server use this
/// seam with a permissive predicate, since every loopback/private address a
/// local test server can bind to is, correctly, always blocked by
/// `is_internal_host` (that guard is exercised end-to-end, with the real
/// predicate, by `worker_refuses_internal_destination`).
async fn deliver_to_matching_guarded(
    client: &reqwest::Client,
    subs: &[(crate::tenant::TenantId, Vec<WebhookSubscription>)],
    ev: &WebhookEvent,
    is_blocked: impl Fn(&str) -> bool,
) {
    let Some((_, tenant_subs)) = subs.iter().find(|(t, _)| *t == ev.tenant_id) else {
        return;
    };
    for sub in tenant_subs.iter().filter(|s| matches(s, &ev.event_type)) {
        let host = match extract_host(&sub.url) {
            Some(h) => h,
            None => {
                eprintln!("{}", serde_json::json!({"webhook_invalid_url": &sub.url}));
                continue;
            }
        };
        if is_blocked(&host) {
            eprintln!("{}", serde_json::json!({"webhook_ssrf_blocked": &sub.url}));
            continue;
        }
        deliver_one(client, sub, ev).await;
    }
}

/// The per-attempt body plus any extra headers to send with it, computed
/// once per delivery (not per retry attempt) by `deliver_one`.
pub(crate) struct OutgoingRequest {
    pub(crate) body: String,
    pub(crate) extra_headers: Vec<(&'static str, String)>,
}

/// Builds the outgoing request for `sub`/`ev`, branching on the
/// subscription kind: `Generic` signs `ev.body` verbatim per Standard
/// Webhooks and adds the three `webhook-*` headers; the native chat kinds
/// (Slack/Discord/Telegram) format a plain-text message from `ev.body` and
/// wrap it in that channel's JSON shape, unsigned, with no extra headers
/// (the receiver authenticates by the secret URL itself). Returns `None`
/// only if signing fails for `Generic` (invalid secret encoding).
///
/// Shared with `api::admin_webhooks_test`, so the "send test event" admin
/// endpoint produces byte-for-byte the same request shape a real delivery
/// would (see review Task 1 of #6: the test-send previously always sent a
/// signed Generic envelope, which is the wrong shape for channel kinds).
/// `id_override` supplies the Standard Webhooks message id (the `webhook-id`
/// header, which is also what the signature is computed over). The in-memory
/// path passes `None` and a fresh random id is generated per delivery; the
/// durable relay passes `Some(delivery_key)` so `webhook-id` is stable across
/// attempts and nodes (the idempotency win) AND the signature stays valid
/// (both header and signed content use the same id). Ignored for channel
/// kinds, which send no `webhook-id`.
pub(crate) fn build_outgoing_request(
    sub: &WebhookSubscription,
    ev: &WebhookEvent,
    id_override: Option<&str>,
) -> Option<OutgoingRequest> {
    match sub.kind {
        SubscriptionKind::Generic => {
            let msg_id = match id_override {
                Some(id) => id.to_string(),
                None => generate_msg_id(),
            };
            let ts = crate::now() as i64;
            let signature = match sign(&sub.secret, &msg_id, ts, &ev.body) {
                Ok(sig) => sig,
                Err(e) => {
                    eprintln!(
                        "{}",
                        serde_json::json!({"webhook_sign_error": e.to_string(), "url": &sub.url})
                    );
                    return None;
                }
            };
            Some(OutgoingRequest {
                body: ev.body.clone(),
                extra_headers: vec![
                    ("webhook-id", msg_id),
                    ("webhook-timestamp", ts.to_string()),
                    ("webhook-signature", signature),
                ],
            })
        }
        kind => {
            let message = format_message(ev.event_type, &ev.body);
            // `channel_payload` only returns `None` for `Generic`, which
            // this branch never sees.
            let body = channel_payload(kind, &message)
                .expect("channel_payload is Some for non-Generic kinds");
            Some(OutgoingRequest {
                body,
                extra_headers: Vec::new(),
            })
        }
    }
}

/// Delivers `ev` to `sub`, retrying up to `DELIVERY_ATTEMPTS` times with
/// exponential backoff + jitter on non-2xx responses or transport errors.
/// Fail-open: exhausting the budget only logs, it never propagates an error.
async fn deliver_one(client: &reqwest::Client, sub: &WebhookSubscription, ev: &WebhookEvent) {
    let Some(req) = build_outgoing_request(sub, ev, None) else {
        return;
    };

    for attempt in 0..DELIVERY_ATTEMPTS {
        let mut builder = client
            .post(&sub.url)
            .header("content-type", "application/json");
        for (name, value) in &req.extra_headers {
            builder = builder.header(*name, value);
        }
        let res = builder.body(req.body.clone()).send().await;

        match res {
            Ok(resp) if resp.status().is_success() => return,
            Ok(resp) => {
                eprintln!(
                    "{}",
                    serde_json::json!({
                        "webhook_delivery_non_2xx": resp.status().as_u16(),
                        "url": &sub.url,
                        "attempt": attempt + 1,
                    })
                );
            }
            Err(e) => {
                eprintln!(
                    "{}",
                    serde_json::json!({
                        "webhook_delivery_error": e.to_string(),
                        "url": &sub.url,
                        "attempt": attempt + 1,
                    })
                );
            }
        }

        if attempt + 1 < DELIVERY_ATTEMPTS {
            tokio::time::sleep(backoff_with_jitter(attempt)).await;
        }
    }

    // `webhook-id` is only present for `Generic` (Standard Webhooks signing);
    // channel kinds have no per-attempt id to report.
    let msg_id = req
        .extra_headers
        .iter()
        .find(|(name, _)| *name == "webhook-id")
        .map(|(_, value)| value.as_str());
    eprintln!(
        "{}",
        serde_json::json!({"webhook_delivery_exhausted": &sub.url, "msg_id": msg_id})
    );
}

/// How often the relay polls the outbox for due deliveries.
pub const RELAY_POLL_INTERVAL_MS: u64 = 1000;
/// Max deliveries claimed per poll (bounds a single relay's per-tick work).
pub const RELAY_BATCH: i64 = 64;
/// Max delivery attempts before a row is dead-lettered (`dead = true`).
pub const MAX_DELIVERY_ATTEMPTS: u32 = 8;
/// Base of the persisted exponential backoff, in seconds: the delay before the
/// n-th retry is `RELAY_BACKOFF_BASE_SECS * 2^(attempts-1)` plus jitter, capped
/// at `RELAY_BACKOFF_CAP_SECS`. Unlike the in-memory worker's millisecond
/// sleeps, this schedule is persisted in `next_attempt_at` and survives
/// restarts, so it spans up to minutes.
const RELAY_BACKOFF_BASE_SECS: u64 = 2;
/// Upper bound on a single backoff interval (seconds).
const RELAY_BACKOFF_CAP_SECS: u64 = 600;

/// Spawns the durable relay (Postgres-only): on a short interval it claims a
/// batch of due deliveries (`SELECT ... FOR UPDATE SKIP LOCKED`, so replicas
/// never double-deliver) and attempts each one, persisting retry/backoff and
/// dead-lettering after `MAX_DELIVERY_ATTEMPTS`. It keeps a subscription
/// snapshot refreshed off a ticker, like `spawn_webhook_worker`. Wired in
/// `main.rs` only when a Postgres backend is configured.
pub fn spawn_webhook_relay(store: Arc<dyn Store>, client: reqwest::Client) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut subs = refresh_relay_snapshot(&store).await;
        let mut poll = tokio::time::interval(Duration::from_millis(RELAY_POLL_INTERVAL_MS));
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut refresh = tokio::time::interval(Duration::from_secs(REFRESH_INTERVAL_SECS));
        refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                _ = poll.tick() => {
                    let now = crate::now();
                    poll_once(&store, &client, &subs, now, RELAY_BATCH, is_internal_host).await;
                }
                _ = refresh.tick() => {
                    subs = refresh_relay_snapshot(&store).await;
                }
            }
        }
    })
}

/// Reads the subscription snapshot the relay resolves claimed deliveries
/// against. On store error, logs and keeps an empty snapshot (a claimed
/// delivery whose subscription is not found is dead-lettered by
/// `deliver_claimed`, so a transient snapshot miss does not silently drop it
/// permanently: the row is only ever dead-lettered against a real, refreshed
/// snapshot on a later tick... see the note in `deliver_claimed`).
///
/// Scoped to `DEFAULT_TENANT` only: it is a same-tenant fast path (ids are
/// globally unique, so a hit here is never a cross-tenant match), not the
/// authoritative source. `deliver_claimed` falls through to
/// `store.get_webhook(delivery.tenant_id, ...)` on a miss, which is correct
/// for every tenant — this snapshot just avoids that DB round-trip for the
/// common (`DEFAULT_TENANT`) case. Multi-tenant load may want an
/// all-tenant snapshot keyed by `(tenant, id)` instead; not needed while P2b
/// has not yet created real tenants.
async fn refresh_relay_snapshot(store: &Arc<dyn Store>) -> Vec<WebhookSubscription> {
    match store.list_webhooks(crate::tenant::DEFAULT_TENANT).await {
        Ok(subs) => subs,
        Err(e) => {
            eprintln!(
                "{}",
                serde_json::json!({"webhook_relay_snapshot_error": e.to_string()})
            );
            Vec::new()
        }
    }
}

/// One relay poll: claims up to `limit` due deliveries and attempts each. The
/// SSRF host-block predicate is injected (`is_blocked`) exactly like
/// `deliver_to_matching_guarded`: production passes `is_internal_host`; the
/// gated integration test passes a permissive predicate so it can drive real
/// delivery against a loopback mock server (which the real guard, correctly,
/// always blocks). Returns the number of rows claimed this poll.
pub async fn poll_once(
    store: &Arc<dyn Store>,
    client: &reqwest::Client,
    subs: &[WebhookSubscription],
    now: u64,
    limit: i64,
    is_blocked: impl Fn(&str) -> bool,
) -> usize {
    let claimed = match store.claim_due_deliveries(now, limit).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "{}",
                serde_json::json!({"webhook_relay_claim_error": e.to_string()})
            );
            return 0;
        }
    };
    let n = claimed.len();
    for delivery in &claimed {
        deliver_claimed(store, client, subs, delivery, &is_blocked, now).await;
    }
    n
}

/// Attempts a single claimed delivery and persists the outcome. Resolves the
/// subscription from `subs`; a subscription deleted since enqueue is
/// dead-lettered (nothing to deliver to). SSRF-guards the destination (a
/// blocked host is dead-lettered: it is undeliverable by policy and would
/// otherwise be re-claimed forever). On a 2xx the row is marked delivered; on
/// any failure `attempts` is incremented and the row is either dead-lettered
/// (at `MAX_DELIVERY_ATTEMPTS`) or rescheduled with persisted exponential
/// backoff. The `webhook-id` header is the persisted `delivery_key`, stable
/// across attempts and nodes.
async fn deliver_claimed(
    store: &Arc<dyn Store>,
    client: &reqwest::Client,
    subs: &[WebhookSubscription],
    delivery: &OutboxDelivery,
    is_blocked: impl Fn(&str) -> bool,
    now: u64,
) {
    let fetched;
    let sub = match subs.iter().find(|s| s.id == delivery.subscription_id) {
        Some(s) => s,
        None => match store
            .get_webhook(delivery.tenant_id, delivery.subscription_id)
            .await
        {
            Ok(Some(s)) => {
                fetched = s;
                &fetched
            }
            Ok(None) => {
                eprintln!(
                    "{}",
                    serde_json::json!({
                        "webhook_relay_sub_deleted": delivery.subscription_id,
                        "delivery_key": &delivery.delivery_key,
                    })
                );
                mark_dead_logged(store, delivery.id, delivery.attempts).await;
                return;
            }
            Err(e) => {
                eprintln!(
                    "{}",
                    serde_json::json!({
                        "webhook_relay_sub_lookup_error": e.to_string(),
                        "delivery_key": &delivery.delivery_key,
                    })
                );
                let next =
                    now.saturating_add(relay_backoff_secs(delivery.attempts.saturating_add(1)));
                let _ = store.mark_retry(delivery.id, next, delivery.attempts).await;
                return;
            }
        },
    };

    let host = match extract_host(&sub.url) {
        Some(h) => h,
        None => {
            eprintln!(
                "{}",
                serde_json::json!({"webhook_relay_invalid_url": &sub.url})
            );
            mark_dead_logged(store, delivery.id, delivery.attempts).await;
            return;
        }
    };
    if is_blocked(&host) {
        eprintln!(
            "{}",
            serde_json::json!({"webhook_relay_ssrf_blocked": &sub.url})
        );
        mark_dead_logged(store, delivery.id, delivery.attempts).await;
        return;
    }

    let Some(event_type) = EventType::from_wire(&delivery.event_type) else {
        eprintln!(
            "{}",
            serde_json::json!({"webhook_relay_bad_event_type": &delivery.event_type})
        );
        mark_dead_logged(store, delivery.id, delivery.attempts).await;
        return;
    };
    let ev = WebhookEvent {
        event_type,
        body: delivery.payload.clone(),
        tenant_id: delivery.tenant_id,
    };
    let Some(req) = build_outgoing_request(sub, &ev, Some(&delivery.delivery_key)) else {
        eprintln!(
            "{}",
            serde_json::json!({"webhook_relay_build_failed": &delivery.delivery_key})
        );
        mark_dead_logged(store, delivery.id, delivery.attempts).await;
        return;
    };

    if post_once(client, sub, &req).await {
        if let Err(e) = store.mark_delivered(delivery.id).await {
            eprintln!(
                "{}",
                serde_json::json!({"webhook_relay_mark_delivered_error": e.to_string()})
            );
        }
        return;
    }

    let attempts = delivery.attempts.saturating_add(1);
    if attempts >= MAX_DELIVERY_ATTEMPTS {
        eprintln!(
            "{}",
            serde_json::json!({
                "webhook_relay_dead_letter": &delivery.delivery_key,
                "attempts": attempts,
            })
        );
        mark_dead_logged(store, delivery.id, attempts).await;
        return;
    }
    let next_attempt_at = now.saturating_add(relay_backoff_secs(attempts));
    if let Err(e) = store
        .mark_retry(delivery.id, next_attempt_at, attempts)
        .await
    {
        eprintln!(
            "{}",
            serde_json::json!({"webhook_relay_mark_retry_error": e.to_string()})
        );
    }
}

/// Sends `req` once (no in-attempt retry: the persisted schedule owns retry).
/// Returns `true` on a 2xx, `false` on a non-2xx or transport error.
async fn post_once(
    client: &reqwest::Client,
    sub: &WebhookSubscription,
    req: &OutgoingRequest,
) -> bool {
    let mut builder = client
        .post(&sub.url)
        .header("content-type", "application/json");
    for (name, value) in &req.extra_headers {
        builder = builder.header(*name, value);
    }
    match builder.body(req.body.clone()).send().await {
        Ok(resp) if resp.status().is_success() => true,
        Ok(resp) => {
            eprintln!(
                "{}",
                serde_json::json!({
                    "webhook_relay_non_2xx": resp.status().as_u16(),
                    "url": &sub.url,
                })
            );
            false
        }
        Err(e) => {
            eprintln!(
                "{}",
                serde_json::json!({"webhook_relay_error": e.to_string(), "url": &sub.url})
            );
            false
        }
    }
}

/// `mark_dead` with its error logged and swallowed (a failed dead-letter is a
/// leased row that will simply be re-claimed and re-tried after the lease).
async fn mark_dead_logged(store: &Arc<dyn Store>, id: i64, attempts: u32) {
    if let Err(e) = store.mark_dead(id, attempts).await {
        eprintln!(
            "{}",
            serde_json::json!({"webhook_relay_mark_dead_error": e.to_string()})
        );
    }
}

/// Persisted backoff for the `attempts`-th retry (`attempts >= 1`):
/// `base * 2^(attempts-1)` seconds, capped, plus up to 50% jitter (so many
/// failing deliveries do not retry in lockstep).
fn relay_backoff_secs(attempts: u32) -> u64 {
    let shift = attempts.saturating_sub(1).min(16);
    let base = RELAY_BACKOFF_BASE_SECS
        .saturating_mul(1u64 << shift)
        .min(RELAY_BACKOFF_CAP_SECS);
    let mut jitter_byte = [0u8; 1];
    let jitter = if getrandom::fill(&mut jitter_byte).is_ok() {
        (jitter_byte[0] as u64) % (base / 2 + 1)
    } else {
        0
    };
    base + jitter
}

/// `msg_<32 hex chars>` from 16 random bytes.
fn generate_msg_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("system RNG must be available");
    let hex = crate::hex(&bytes);
    format!("msg_{hex}")
}

/// `base * 2^attempt` plus up to 50% jitter.
fn backoff_with_jitter(attempt: u32) -> Duration {
    let base = BACKOFF_BASE_MS.saturating_mul(1u64 << attempt.min(16));
    let mut jitter_byte = [0u8; 1];
    let jitter = if getrandom::fill(&mut jitter_byte).is_ok() {
        (jitter_byte[0] as u64) % (base / 2 + 1)
    } else {
        0
    };
    Duration::from_millis(base + jitter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Record;
    use crate::webhooks::EventType;
    use axum::body::Bytes;
    use axum::extract::State;
    use axum::http::HeaderMap;
    use axum::routing::any;
    use axum::Router;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex;
    use tokio::net::TcpListener;

    /// Captured request: headers (lowercased names) + raw body.
    #[derive(Debug, Clone)]
    struct Captured {
        headers: std::collections::HashMap<String, String>,
        body: String,
    }

    /// Shared test-server state: every captured POST, plus an optional
    /// sequence of status codes to reply with in order (repeats the last
    /// one once exhausted).
    struct ServerState {
        captured: Mutex<Vec<Captured>>,
        responses: Vec<u16>,
        next: AtomicUsize,
    }

    async fn handler(
        State(state): State<Arc<ServerState>>,
        headers: HeaderMap,
        body: Bytes,
    ) -> axum::http::StatusCode {
        let mut map = std::collections::HashMap::new();
        for (k, v) in headers.iter() {
            map.insert(
                k.as_str().to_ascii_lowercase(),
                v.to_str().unwrap().to_string(),
            );
        }
        state.captured.lock().unwrap().push(Captured {
            headers: map,
            body: String::from_utf8(body.to_vec()).unwrap(),
        });
        let idx = state.next.fetch_add(1, Ordering::SeqCst);
        let code = state
            .responses
            .get(idx)
            .copied()
            .unwrap_or(*state.responses.last().unwrap());
        axum::http::StatusCode::from_u16(code).unwrap()
    }

    /// Spins a local server replying with `responses` in sequence (repeating
    /// the last entry). Returns the base URL and the shared state to inspect.
    async fn spawn_test_server(responses: Vec<u16>) -> (String, Arc<ServerState>) {
        let state = Arc::new(ServerState {
            captured: Mutex::new(Vec::new()),
            responses,
            next: AtomicUsize::new(0),
        });
        let app = Router::new()
            .route("/hook", any(handler))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}/hook"), state)
    }

    /// Minimal `Store` stub: only `list_webhooks`/`list_tenants` are
    /// exercised by the delivery worker; every other method is unreachable
    /// from these tests. Subscriptions are keyed by tenant so multi-tenant
    /// snapshot/isolation tests (LUC-63) can give each tenant its own set;
    /// `list_tenants` returns exactly the keys present. `seen_tenant` records
    /// the LAST tenant `list_webhooks` was called with, so single-tenant
    /// tests can assert a caller threaded the right tenant through.
    /// `fail` lets a test flip the stub to erroring after a good snapshot has
    /// already been read, for the `refresh_snapshot` fail-open test.
    struct StubStore {
        subs_by_tenant:
            std::collections::HashMap<crate::tenant::TenantId, Vec<WebhookSubscription>>,
        seen_tenant: std::sync::Mutex<Option<crate::tenant::TenantId>>,
        fail: std::sync::atomic::AtomicBool,
    }

    impl StubStore {
        /// Single-tenant convenience constructor: `subs` all belong to
        /// `DEFAULT_TENANT` (the shape every pre-LUC-63 test uses).
        fn new(subs: Vec<WebhookSubscription>) -> Self {
            Self::new_multi(vec![(crate::tenant::DEFAULT_TENANT, subs)])
        }

        /// Multi-tenant constructor: each `(tenant, subs)` pair becomes both
        /// one entry `list_tenants` returns and that tenant's `list_webhooks`
        /// result. Used by the LUC-63 isolation/gate tests.
        fn new_multi(pairs: Vec<(crate::tenant::TenantId, Vec<WebhookSubscription>)>) -> Self {
            Self {
                subs_by_tenant: pairs.into_iter().collect(),
                seen_tenant: std::sync::Mutex::new(None),
                fail: std::sync::atomic::AtomicBool::new(false),
            }
        }

        /// Flips this stub to erroring `list_webhooks` calls (used by the
        /// `refresh_snapshot` fail-open test, after a first successful
        /// snapshot has already been taken).
        fn set_fail(&self, fail: bool) {
            self.fail.store(fail, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[async_trait::async_trait]
    impl Store for StubStore {
        async fn next_id(&self, _tenant: crate::tenant::TenantId) -> Result<u64, StoreError> {
            unimplemented!()
        }
        async fn get_link(
            &self,
            _tenant: crate::tenant::TenantId,
            _id: u64,
        ) -> Result<Option<Record>, StoreError> {
            unimplemented!()
        }
        async fn put_link(
            &self,
            _tenant: crate::tenant::TenantId,
            _id: u64,
            _rec: &Record,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn get_alias(
            &self,
            _domain_id: u64,
            _alias: &str,
        ) -> Result<Option<u64>, StoreError> {
            unimplemented!()
        }
        async fn put_alias_and_link(
            &self,
            _tenant: crate::tenant::TenantId,
            _domain_id: u64,
            _alias: &str,
            _id: u64,
            _rec: &Record,
        ) -> Result<bool, StoreError> {
            unimplemented!()
        }
        async fn put_link_tx(
            &self,
            _tenant: crate::tenant::TenantId,
            _id: u64,
            _rec: &Record,
            _deliveries: &[OutboxRow],
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn put_alias_and_link_tx(
            &self,
            _tenant: crate::tenant::TenantId,
            _domain_id: u64,
            _alias: &str,
            _id: u64,
            _rec: &Record,
            _deliveries: &[OutboxRow],
        ) -> Result<bool, StoreError> {
            unimplemented!()
        }
        async fn delete_link_tx(
            &self,
            _tenant: crate::tenant::TenantId,
            _id: u64,
            _deliveries: &[OutboxRow],
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn list_links(
            &self,
            _tenant: crate::tenant::TenantId,
            _after: Option<u64>,
            _limit: usize,
            _tag: Option<&str>,
            _folder: Option<&str>,
            _active_only: bool,
        ) -> Result<Vec<(u64, Record)>, StoreError> {
            unimplemented!()
        }
        #[allow(clippy::too_many_arguments)]
        async fn search_links(
            &self,
            _tenant: crate::tenant::TenantId,
            _q: &str,
            _after: Option<u64>,
            _limit: usize,
            _tag: Option<&str>,
            _folder: Option<&str>,
            _active_only: bool,
        ) -> Result<Vec<(u64, Record)>, StoreError> {
            unimplemented!()
        }
        async fn list_tags(
            &self,
            _tenant: crate::tenant::TenantId,
        ) -> Result<Vec<(String, u64)>, StoreError> {
            unimplemented!()
        }
        async fn list_folders(
            &self,
            _tenant: crate::tenant::TenantId,
        ) -> Result<Vec<(String, u64)>, StoreError> {
            unimplemented!()
        }
        async fn list_aliases(
            &self,
            _tenant: crate::tenant::TenantId,
        ) -> Result<Vec<(String, u64)>, StoreError> {
            unimplemented!()
        }
        async fn delete_link(
            &self,
            _tenant: crate::tenant::TenantId,
            _id: u64,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn delete_alias(
            &self,
            _tenant: crate::tenant::TenantId,
            _alias: &str,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn list_webhooks(
            &self,
            tenant: crate::tenant::TenantId,
        ) -> Result<Vec<WebhookSubscription>, StoreError> {
            if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(StoreError::Backend("stub list_webhooks failure".into()));
            }
            *self.seen_tenant.lock().unwrap() = Some(tenant);
            Ok(self
                .subs_by_tenant
                .get(&tenant)
                .cloned()
                .unwrap_or_default())
        }
        async fn get_webhook(
            &self,
            _tenant: crate::tenant::TenantId,
            _id: u64,
        ) -> Result<Option<WebhookSubscription>, StoreError> {
            unimplemented!()
        }
        async fn put_webhook(
            &self,
            _tenant: crate::tenant::TenantId,
            _sub: &WebhookSubscription,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn delete_webhook(
            &self,
            _tenant: crate::tenant::TenantId,
            _id: u64,
        ) -> Result<bool, StoreError> {
            unimplemented!()
        }
        async fn next_webhook_id(
            &self,
            _tenant: crate::tenant::TenantId,
        ) -> Result<u64, StoreError> {
            unimplemented!()
        }
        async fn put_alert_rule(
            &self,
            _tenant: crate::tenant::TenantId,
            _link_id: u64,
            _rule: &crate::store::AlertRule,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn get_alert_rule(
            &self,
            _tenant: crate::tenant::TenantId,
            _link_id: u64,
        ) -> Result<Option<crate::store::AlertRule>, StoreError> {
            unimplemented!()
        }
        async fn delete_alert_rule(
            &self,
            _tenant: crate::tenant::TenantId,
            _link_id: u64,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn list_alert_rules(
            &self,
            _tenant: crate::tenant::TenantId,
        ) -> Result<Vec<(u64, crate::store::AlertRule)>, StoreError> {
            unimplemented!()
        }
        async fn list_api_tokens(
            &self,
            _tenant: crate::tenant::TenantId,
        ) -> Result<Vec<crate::auth::ApiToken>, StoreError> {
            unimplemented!()
        }
        async fn get_api_token_by_hash(
            &self,
            _hash: &str,
        ) -> Result<Option<crate::auth::ApiToken>, StoreError> {
            unimplemented!()
        }
        async fn put_api_token(
            &self,
            _tenant: crate::tenant::TenantId,
            _token: &crate::auth::ApiToken,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn delete_api_token(
            &self,
            _tenant: crate::tenant::TenantId,
            _id: u64,
        ) -> Result<bool, StoreError> {
            unimplemented!()
        }
        async fn next_api_token_id(
            &self,
            _tenant: crate::tenant::TenantId,
        ) -> Result<u64, StoreError> {
            unimplemented!()
        }
        async fn bump_visits(
            &self,
            _tenant: crate::tenant::TenantId,
            _id: u64,
        ) -> Result<u64, StoreError> {
            unimplemented!()
        }
        async fn visits(
            &self,
            _tenant: crate::tenant::TenantId,
            _id: u64,
        ) -> Result<u64, StoreError> {
            unimplemented!()
        }
        async fn visits_for(
            &self,
            _tenant: crate::tenant::TenantId,
            _ids: &[u64],
        ) -> Result<std::collections::HashMap<u64, u64>, StoreError> {
            unimplemented!()
        }
        async fn put_link_health(
            &self,
            _tenant: crate::tenant::TenantId,
            _id: u64,
            _health: &crate::store::LinkHealth,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn list_link_health(
            &self,
            _tenant: crate::tenant::TenantId,
        ) -> Result<Vec<(u64, crate::store::LinkHealth)>, StoreError> {
            unimplemented!()
        }
        async fn link_health_for(
            &self,
            _tenant: crate::tenant::TenantId,
            _ids: &[u64],
        ) -> Result<Vec<(u64, crate::store::LinkHealth)>, StoreError> {
            unimplemented!()
        }
        async fn list_broken_link_ids(
            &self,
            _tenant: crate::tenant::TenantId,
        ) -> Result<Vec<u64>, StoreError> {
            unimplemented!()
        }
        async fn try_acquire_health_lease(
            &self,
            _holder: &str,
            _ttl_secs: u64,
        ) -> Result<bool, StoreError> {
            unimplemented!()
        }
        async fn put_sheets_connection(
            &self,
            _tenant: crate::tenant::TenantId,
            _c: &crate::sheets::SheetsConnection,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn get_sheets_connection(
            &self,
            _tenant: crate::tenant::TenantId,
        ) -> Result<Option<crate::sheets::SheetsConnection>, StoreError> {
            unimplemented!()
        }
        async fn delete_sheets_connection(
            &self,
            _tenant: crate::tenant::TenantId,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn try_acquire_sheets_lease(
            &self,
            _holder: &str,
            _ttl_secs: u64,
        ) -> Result<bool, StoreError> {
            unimplemented!()
        }
        async fn put_session(
            &self,
            _tenant: crate::tenant::TenantId,
            _session: &crate::auth::Session,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn get_session_by_hash(
            &self,
            _token_hash: &str,
            _now: u64,
        ) -> Result<Option<crate::auth::Session>, StoreError> {
            unimplemented!()
        }
        async fn delete_session(&self, _token_hash: &str) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn gc_sessions(&self, _now: u64) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn purge_click_events_before(&self, _cutoff_ts: u64) -> Result<u64, StoreError> {
            unimplemented!()
        }
        async fn delete_link_analytics(
            &self,
            _tenant: crate::tenant::TenantId,
            _id: u64,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn next_pixel_id(&self, _tenant: crate::tenant::TenantId) -> Result<u64, StoreError> {
            unimplemented!()
        }
        async fn get_pixel(
            &self,
            _tenant: crate::tenant::TenantId,
            _id: u64,
        ) -> Result<Option<crate::pixel::PixelConfig>, StoreError> {
            unimplemented!()
        }
        async fn put_pixel(
            &self,
            _tenant: crate::tenant::TenantId,
            _config: &crate::pixel::PixelConfig,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn delete_pixel(
            &self,
            _tenant: crate::tenant::TenantId,
            _id: u64,
        ) -> Result<bool, StoreError> {
            unimplemented!()
        }
        async fn list_pixels(
            &self,
            _tenant: crate::tenant::TenantId,
        ) -> Result<Vec<crate::pixel::PixelConfig>, StoreError> {
            unimplemented!()
        }
        async fn get_wellknown(
            &self,
            _tenant: crate::tenant::TenantId,
            _name: &str,
        ) -> Result<Option<String>, StoreError> {
            unimplemented!()
        }
        async fn put_wellknown(
            &self,
            _tenant: crate::tenant::TenantId,
            _name: &str,
            _body: &str,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn delete_wellknown(
            &self,
            _tenant: crate::tenant::TenantId,
            _name: &str,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn put_tenant(&self, _t: &crate::tenant::Tenant) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn get_tenant(
            &self,
            _id: crate::tenant::TenantId,
        ) -> Result<Option<crate::tenant::Tenant>, StoreError> {
            unimplemented!()
        }
        async fn list_tenants(&self) -> Result<Vec<crate::tenant::Tenant>, StoreError> {
            Ok(self
                .subs_by_tenant
                .keys()
                .map(|id| crate::tenant::Tenant {
                    id: *id,
                    name: format!("tenant-{}", id.0),
                    slug: format!("t{}", id.0),
                    created: 0,
                })
                .collect())
        }
        async fn get_tenant_by_slug(
            &self,
            _slug: &str,
        ) -> Result<Option<crate::tenant::Tenant>, StoreError> {
            unimplemented!()
        }
        async fn next_user_id(&self) -> Result<u64, StoreError> {
            unimplemented!()
        }
        async fn next_tenant_id(&self) -> Result<u64, StoreError> {
            unimplemented!()
        }
        async fn put_user(&self, _u: &crate::tenant::User) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn get_user_by_subject(
            &self,
            _subject: &str,
        ) -> Result<Option<crate::tenant::User>, StoreError> {
            unimplemented!()
        }
        async fn get_user_by_id(
            &self,
            _id: u64,
        ) -> Result<Option<crate::tenant::User>, StoreError> {
            unimplemented!()
        }
        async fn put_membership(&self, _m: &crate::tenant::Membership) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn get_membership(
            &self,
            _user_id: u64,
            _tenant: crate::tenant::TenantId,
        ) -> Result<Option<crate::tenant::Membership>, StoreError> {
            unimplemented!()
        }
        async fn list_memberships_for_user(
            &self,
            _user_id: u64,
        ) -> Result<Vec<crate::tenant::Membership>, StoreError> {
            unimplemented!()
        }
        async fn get_owner_user_id(
            &self,
            _tenant: crate::tenant::TenantId,
        ) -> Result<Option<u64>, StoreError> {
            unimplemented!()
        }
        async fn next_domain_id(&self) -> Result<u64, StoreError> {
            unimplemented!()
        }
        async fn get_domain_by_host(
            &self,
            _host: &str,
        ) -> Result<Option<crate::domain::Domain>, StoreError> {
            unimplemented!()
        }
        async fn get_domain(
            &self,
            _tenant: crate::tenant::TenantId,
            _id: u64,
        ) -> Result<Option<crate::domain::Domain>, StoreError> {
            unimplemented!()
        }
        async fn list_domains(
            &self,
            _tenant: crate::tenant::TenantId,
        ) -> Result<Vec<crate::domain::Domain>, StoreError> {
            unimplemented!()
        }
        async fn put_domain(&self, _domain: &crate::domain::Domain) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn set_domain_status(
            &self,
            _tenant: crate::tenant::TenantId,
            _id: u64,
            _status: crate::domain::DomainStatus,
            _verified_at: Option<u64>,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn delete_domain(
            &self,
            _tenant: crate::tenant::TenantId,
            _id: u64,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn next_sso_domain_id(&self) -> Result<u64, StoreError> {
            unimplemented!()
        }
        async fn get_sso_domain_bare(
            &self,
            _domain: &str,
        ) -> Result<Option<crate::sso::SsoEmailDomain>, StoreError> {
            unimplemented!()
        }
        async fn get_sso_domain(
            &self,
            _tenant: crate::tenant::TenantId,
            _id: u64,
        ) -> Result<Option<crate::sso::SsoEmailDomain>, StoreError> {
            unimplemented!()
        }
        async fn list_sso_domains(
            &self,
            _tenant: crate::tenant::TenantId,
        ) -> Result<Vec<crate::sso::SsoEmailDomain>, StoreError> {
            unimplemented!()
        }
        async fn put_sso_domain(
            &self,
            _domain: &crate::sso::SsoEmailDomain,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn set_sso_domain_status(
            &self,
            _tenant: crate::tenant::TenantId,
            _id: u64,
            _status: crate::domain::DomainStatus,
            _verified_at: Option<u64>,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn delete_sso_domain(
            &self,
            _tenant: crate::tenant::TenantId,
            _id: u64,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn next_invite_id(&self) -> Result<u64, StoreError> {
            unimplemented!()
        }
        async fn create_invite(&self, _inv: &crate::invite::Invite) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn get_invite_by_hash(
            &self,
            _token_hash: &str,
            _now: u64,
        ) -> Result<Option<crate::invite::Invite>, StoreError> {
            unimplemented!()
        }
        async fn mark_invite_accepted(
            &self,
            _id: u64,
            _accepted_by: u64,
            _now: u64,
        ) -> Result<bool, StoreError> {
            unimplemented!()
        }
        async fn accept_invite_tx(
            &self,
            _invite_id: u64,
            _membership: &crate::tenant::Membership,
            _now: u64,
        ) -> Result<bool, StoreError> {
            unimplemented!()
        }
        async fn list_invites(
            &self,
            _tenant: crate::tenant::TenantId,
        ) -> Result<Vec<crate::invite::Invite>, StoreError> {
            unimplemented!()
        }
        async fn delete_invite(
            &self,
            _tenant: crate::tenant::TenantId,
            _id: u64,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn next_oidc_config_id(&self) -> Result<u64, StoreError> {
            unimplemented!()
        }
        async fn put_oidc_config(
            &self,
            _cfg: &crate::oidc::TenantOidcConfig,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn get_oidc_config(
            &self,
            _tenant: crate::tenant::TenantId,
        ) -> Result<Option<crate::oidc::TenantOidcConfig>, StoreError> {
            unimplemented!()
        }
        async fn get_oidc_config_bare(
            &self,
            _tenant: crate::tenant::TenantId,
        ) -> Result<Option<crate::oidc::TenantOidcConfig>, StoreError> {
            unimplemented!()
        }
        async fn delete_oidc_config(
            &self,
            _tenant: crate::tenant::TenantId,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn update_oidc_config_member_value(
            &self,
            _tenant: crate::tenant::TenantId,
            _member_value: &str,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn update_oidc_config_issuer(
            &self,
            _tenant: crate::tenant::TenantId,
            _issuer: &str,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn enqueue_deliveries(&self, _rows: &[OutboxRow]) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn claim_due_deliveries(
            &self,
            _now: u64,
            _limit: i64,
        ) -> Result<Vec<OutboxDelivery>, StoreError> {
            unimplemented!()
        }
        async fn mark_delivered(&self, _id: i64) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn mark_retry(
            &self,
            _id: i64,
            _next_attempt_at: u64,
            _attempts: u32,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn mark_dead(&self, _id: i64, _attempts: u32) -> Result<(), StoreError> {
            unimplemented!()
        }
    }

    fn sub(
        id: u64,
        url: &str,
        events: Vec<EventType>,
        active: bool,
        secret: &str,
    ) -> WebhookSubscription {
        WebhookSubscription {
            id,
            url: url.to_string(),
            events,
            secret: secret.to_string(),
            active,
            created: 0,
            kind: SubscriptionKind::Generic,
        }
    }

    /// Exercises real HTTP delivery (matching, signing, headers) against a
    /// local test server via the guarded seam (see
    /// `deliver_to_matching_guarded`'s doc comment for why: every address a
    /// local server can bind to is a loopback/private address, which the
    /// production `is_internal_host` guard correctly always blocks; that
    /// guard itself is verified end-to-end by
    /// `worker_refuses_internal_destination`).
    #[tokio::test]
    async fn worker_delivers_signed_matching_event() {
        let (url, state) = spawn_test_server(vec![200]).await;
        let secret = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw".to_string();
        let subs = vec![(
            crate::tenant::DEFAULT_TENANT,
            vec![sub(1, &url, vec![EventType::LinkCreated], true, &secret)],
        )];
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(DELIVERY_TIMEOUT_SECS))
            .redirect(Policy::none())
            .build()
            .unwrap();
        let body = r#"{"test":2432232314}"#.to_string();
        let ev = WebhookEvent {
            event_type: EventType::LinkCreated,
            body: body.clone(),
            tenant_id: crate::tenant::DEFAULT_TENANT,
        };

        deliver_to_matching_guarded(&client, &subs, &ev, |_| false).await;

        let captured = state.captured.lock().unwrap();
        assert_eq!(captured.len(), 1);
        let req = &captured[0];
        assert_eq!(req.body, body);
        let msg_id = req.headers.get("webhook-id").expect("webhook-id header");
        let ts: i64 = req
            .headers
            .get("webhook-timestamp")
            .expect("webhook-timestamp header")
            .parse()
            .unwrap();
        let sig = req
            .headers
            .get("webhook-signature")
            .expect("webhook-signature header");
        let expected = sign(&secret, msg_id, ts, &body).unwrap();
        assert_eq!(sig, &expected);
    }

    /// A Slack-kind subscription must receive the formatted `{"text": ...}`
    /// payload (not `ev.body` verbatim) and must NOT carry any of the
    /// Standard Webhooks signing headers: the receiving Slack incoming
    /// webhook authenticates by the secret URL itself, so signing would be
    /// meaningless (and would leak nothing useful to a Slack client anyway).
    #[tokio::test]
    async fn worker_delivers_slack_payload_unsigned() {
        let (url, state) = spawn_test_server(vec![200]).await;
        let mut slack_sub = sub(1, &url, vec![EventType::LinkCreated], true, "");
        slack_sub.kind = SubscriptionKind::Slack;
        let subs = vec![(crate::tenant::DEFAULT_TENANT, vec![slack_sub])];
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(DELIVERY_TIMEOUT_SECS))
            .redirect(Policy::none())
            .build()
            .unwrap();
        let body =
            r#"{"type":"link.created","data":{"code":"abc123","url":"https://e.com"}}"#.to_string();
        let ev = WebhookEvent {
            event_type: EventType::LinkCreated,
            body,
            tenant_id: crate::tenant::DEFAULT_TENANT,
        };

        deliver_to_matching_guarded(&client, &subs, &ev, |_| false).await;

        let captured = state.captured.lock().unwrap();
        assert_eq!(captured.len(), 1);
        let req = &captured[0];
        assert_eq!(
            req.body,
            r#"{"text":"New short link: abc123 -> https://e.com"}"#
        );
        assert!(!req.headers.contains_key("webhook-signature"));
        assert!(!req.headers.contains_key("webhook-id"));
        assert!(!req.headers.contains_key("webhook-timestamp"));
    }

    /// A Discord-kind subscription must receive the formatted
    /// `{"content": ...}` payload (Discord's shape, not Slack/Telegram's
    /// `{"text": ...}`) and must NOT carry any Standard Webhooks signing
    /// headers, for the same reason as Slack: the incoming webhook URL is
    /// the authentication.
    #[tokio::test]
    async fn worker_delivers_discord_payload_unsigned() {
        let (url, state) = spawn_test_server(vec![200]).await;
        let mut discord_sub = sub(1, &url, vec![EventType::LinkCreated], true, "");
        discord_sub.kind = SubscriptionKind::Discord;
        let subs = vec![(crate::tenant::DEFAULT_TENANT, vec![discord_sub])];
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(DELIVERY_TIMEOUT_SECS))
            .redirect(Policy::none())
            .build()
            .unwrap();
        let body =
            r#"{"type":"link.created","data":{"code":"abc123","url":"https://e.com"}}"#.to_string();
        let ev = WebhookEvent {
            event_type: EventType::LinkCreated,
            body,
            tenant_id: crate::tenant::DEFAULT_TENANT,
        };

        deliver_to_matching_guarded(&client, &subs, &ev, |_| false).await;

        let captured = state.captured.lock().unwrap();
        assert_eq!(captured.len(), 1);
        let req = &captured[0];
        assert_eq!(
            req.body,
            r#"{"content":"New short link: abc123 -> https://e.com"}"#
        );
        assert!(!req.headers.contains_key("webhook-signature"));
        assert!(!req.headers.contains_key("webhook-id"));
        assert!(!req.headers.contains_key("webhook-timestamp"));
    }

    /// A Telegram-kind subscription must receive the formatted
    /// `{"text": ...}` payload (same shape as Slack) and must NOT carry any
    /// Standard Webhooks signing headers, for the same reason as Slack: the
    /// incoming webhook URL is the authentication.
    #[tokio::test]
    async fn worker_delivers_telegram_payload_unsigned() {
        let (url, state) = spawn_test_server(vec![200]).await;
        let mut telegram_sub = sub(1, &url, vec![EventType::LinkCreated], true, "");
        telegram_sub.kind = SubscriptionKind::Telegram;
        let subs = vec![(crate::tenant::DEFAULT_TENANT, vec![telegram_sub])];
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(DELIVERY_TIMEOUT_SECS))
            .redirect(Policy::none())
            .build()
            .unwrap();
        let body =
            r#"{"type":"link.created","data":{"code":"abc123","url":"https://e.com"}}"#.to_string();
        let ev = WebhookEvent {
            event_type: EventType::LinkCreated,
            body,
            tenant_id: crate::tenant::DEFAULT_TENANT,
        };

        deliver_to_matching_guarded(&client, &subs, &ev, |_| false).await;

        let captured = state.captured.lock().unwrap();
        assert_eq!(captured.len(), 1);
        let req = &captured[0];
        assert_eq!(
            req.body,
            r#"{"text":"New short link: abc123 -> https://e.com"}"#
        );
        assert!(!req.headers.contains_key("webhook-signature"));
        assert!(!req.headers.contains_key("webhook-id"));
        assert!(!req.headers.contains_key("webhook-timestamp"));
    }

    /// Matching is enforced regardless of the SSRF guard: an inactive
    /// subscription and one subscribed to a different event type must both
    /// be skipped, with zero POSTs, even though the guard here is
    /// permissive (`|_| false`) so a false pass couldn't hide behind
    /// `is_internal_host` blocking the local test server instead.
    #[tokio::test]
    async fn worker_skips_non_matching_and_inactive() {
        let (url, state) = spawn_test_server(vec![200]).await;
        let subs = vec![(
            crate::tenant::DEFAULT_TENANT,
            vec![
                sub(
                    1,
                    &url,
                    vec![EventType::LinkDeleted],
                    true,
                    "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw",
                ),
                sub(
                    2,
                    &url,
                    vec![EventType::LinkCreated],
                    false,
                    "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw",
                ),
            ],
        )];
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(DELIVERY_TIMEOUT_SECS))
            .redirect(Policy::none())
            .build()
            .unwrap();
        let ev = WebhookEvent {
            event_type: EventType::LinkCreated,
            body: "{}".to_string(),
            tenant_id: crate::tenant::DEFAULT_TENANT,
        };

        deliver_to_matching_guarded(&client, &subs, &ev, |_| false).await;

        assert_eq!(state.captured.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn worker_refuses_internal_destination() {
        let store: Arc<dyn Store> = Arc::new(StubStore::new(vec![sub(
            1,
            "http://127.0.0.1:9/hook",
            vec![EventType::LinkCreated],
            true,
            "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw",
        )]));
        let clicked = Arc::new(AtomicBool::new(false));
        let expired = Arc::new(AtomicBool::new(false));
        let (tx, rx) = tokio::sync::mpsc::channel(WEBHOOK_CHANNEL_CAPACITY);
        let dispatcher = WebhookDispatcher::new(tx, clicked, expired);
        let _handle = spawn_webhook_worker(
            rx,
            store,
            dispatcher.clicked_subscribed.clone(),
            dispatcher.expired_subscribed.clone(),
        );

        dispatcher.emit(WebhookEvent {
            event_type: EventType::LinkCreated,
            body: "{}".to_string(),
            tenant_id: crate::tenant::DEFAULT_TENANT,
        });

        // No server is listening on 127.0.0.1:9 (discard port): if the
        // SSRF guard failed to skip, the POST would hang/error against a
        // closed port rather than silently succeed; give it a moment then
        // just assert the worker is still alive (no panic) and did not
        // need any real delivery.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!_handle.is_finished());
    }

    #[tokio::test]
    async fn worker_retries_then_succeeds() {
        let (url, state) = spawn_test_server(vec![500, 200]).await;
        let subs = vec![(
            crate::tenant::DEFAULT_TENANT,
            vec![sub(
                1,
                &url,
                vec![EventType::LinkCreated],
                true,
                "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw",
            )],
        )];
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(DELIVERY_TIMEOUT_SECS))
            .redirect(Policy::none())
            .build()
            .unwrap();
        let ev = WebhookEvent {
            event_type: EventType::LinkCreated,
            body: "{}".to_string(),
            tenant_id: crate::tenant::DEFAULT_TENANT,
        };

        deliver_to_matching_guarded(&client, &subs, &ev, |_| false).await;

        assert_eq!(state.captured.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn refresh_snapshot_sets_clicked_and_expired_flags() {
        let store: Arc<dyn Store> = Arc::new(StubStore::new(vec![
            sub(
                1,
                "https://x",
                vec![EventType::LinkClicked],
                true,
                "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw",
            ),
            sub(
                2,
                "https://y",
                vec![EventType::LinkExpired],
                false,
                "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw",
            ),
        ]));
        let clicked = Arc::new(AtomicBool::new(false));
        let expired = Arc::new(AtomicBool::new(false));
        let mut snapshot = Vec::new();
        refresh_snapshot(&store, &clicked, &expired, &mut snapshot).await;
        assert_eq!(
            snapshot.len(),
            1,
            "single tenant group in OSS/single-tenant mode"
        );
        assert_eq!(snapshot[0].0, crate::tenant::DEFAULT_TENANT);
        assert_eq!(snapshot[0].1.len(), 2);
        assert!(clicked.load(Ordering::Relaxed));
        // sub 2 is inactive, so `expired` must stay false.
        assert!(!expired.load(Ordering::Relaxed));
    }

    /// LUC-63 review fail-open test: a store error on a REFRESH (after a
    /// first snapshot already succeeded) must leave the previous snapshot
    /// and the `clicked`/`expired` gates untouched, never empty them out.
    /// This is the fail-open contract `refresh_snapshot`'s doc-comment
    /// promises: mirrors `analytics::refresh_pixel_snapshot`'s behavior.
    #[tokio::test]
    async fn refresh_snapshot_keeps_previous_on_store_error() {
        let store = Arc::new(StubStore::new(vec![sub(
            1,
            "https://x",
            vec![EventType::LinkClicked],
            true,
            "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw",
        )]));
        let dyn_store: Arc<dyn Store> = store.clone();
        let clicked = Arc::new(AtomicBool::new(false));
        let expired = Arc::new(AtomicBool::new(false));

        let mut snapshot = Vec::new();
        refresh_snapshot(&dyn_store, &clicked, &expired, &mut snapshot).await;
        assert_eq!(
            snapshot.len(),
            1,
            "first refresh must populate the snapshot"
        );
        assert_eq!(snapshot[0].1.len(), 1);
        assert!(clicked.load(Ordering::Relaxed));

        // Simulate a transient store error (or timeout) on the next refresh.
        store.set_fail(true);
        refresh_snapshot(&dyn_store, &clicked, &expired, &mut snapshot).await;

        assert_eq!(
            snapshot.len(),
            1,
            "a store error must leave the previous snapshot untouched, not empty it"
        );
        assert_eq!(snapshot[0].1.len(), 1);
        assert!(
            clicked.load(Ordering::Relaxed),
            "the clicked gate must not be reset by a failed refresh"
        );
        assert!(!expired.load(Ordering::Relaxed));
    }

    /// LUC-63 gate test: a `link.clicked` subscription that exists ONLY in a
    /// non-default tenant must still set the any-tenant `clicked_subscribed`
    /// atomic. Before LUC-63 the worker only ever looked at
    /// `DEFAULT_TENANT`'s subscriptions, so this would incorrectly stay
    /// false.
    #[tokio::test]
    async fn refresh_snapshot_gate_is_any_tenant() {
        let tenant_a = crate::tenant::DEFAULT_TENANT;
        let tenant_b = crate::tenant::TenantId(1);
        let store: Arc<dyn Store> = Arc::new(StubStore::new_multi(vec![
            (tenant_a, vec![]),
            (
                tenant_b,
                vec![sub(
                    1,
                    "https://tenant-b.example/hook",
                    vec![EventType::LinkClicked],
                    true,
                    "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw",
                )],
            ),
        ]));
        let clicked = Arc::new(AtomicBool::new(false));
        let expired = Arc::new(AtomicBool::new(false));
        let mut snapshot = Vec::new();
        refresh_snapshot(&store, &clicked, &expired, &mut snapshot).await;
        assert_eq!(snapshot.len(), 2);
        assert!(
            clicked.load(Ordering::Relaxed),
            "clicked_subscribed must be true: tenant 1 has an active LinkClicked sub"
        );
        assert!(!expired.load(Ordering::Relaxed));
    }

    /// LUC-63 isolation test: two tenants each have an active `link.clicked`
    /// subscription pointed at their OWN mock server. Delivering an event
    /// stamped `tenant_id = 1` must reach only tenant 1's server, never
    /// tenant 0's (a cross-tenant leak would show up as a second capture on
    /// the wrong server).
    #[tokio::test]
    async fn deliver_to_matching_isolates_by_tenant() {
        let (url_a, state_a) = spawn_test_server(vec![200]).await;
        let (url_b, state_b) = spawn_test_server(vec![200]).await;
        let tenant_a = crate::tenant::DEFAULT_TENANT;
        let tenant_b = crate::tenant::TenantId(1);
        let secret = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw";
        let subs = vec![
            (
                tenant_a,
                vec![sub(1, &url_a, vec![EventType::LinkClicked], true, secret)],
            ),
            (
                tenant_b,
                vec![sub(2, &url_b, vec![EventType::LinkClicked], true, secret)],
            ),
        ];
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(DELIVERY_TIMEOUT_SECS))
            .redirect(Policy::none())
            .build()
            .unwrap();
        let ev = WebhookEvent {
            event_type: EventType::LinkClicked,
            body: "{}".to_string(),
            tenant_id: tenant_b,
        };

        deliver_to_matching_guarded(&client, &subs, &ev, |_| false).await;

        assert_eq!(
            state_b.captured.lock().unwrap().len(),
            1,
            "tenant 1's subscription must receive the event"
        );
        assert_eq!(
            state_a.captured.lock().unwrap().len(),
            0,
            "tenant 0's subscription must NOT receive tenant 1's event"
        );
    }

    #[test]
    fn emit_drops_when_channel_full() {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let dispatcher = WebhookDispatcher::new(
            tx,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        );
        dispatcher.emit(WebhookEvent {
            event_type: EventType::LinkCreated,
            body: "a".to_string(),
            tenant_id: crate::tenant::DEFAULT_TENANT,
        });
        // Second emit should be dropped (fail-open), not panic or block.
        dispatcher.emit(WebhookEvent {
            event_type: EventType::LinkCreated,
            body: "b".to_string(),
            tenant_id: crate::tenant::DEFAULT_TENANT,
        });
        drop(rx);
    }

    /// On the outbox backend, `lifecycle_deliveries` reads matching active
    /// subscriptions and returns one row per match (stable `delivery_key`),
    /// WITHOUT touching the in-memory channel and WITHOUT enqueuing.
    #[tokio::test]
    async fn lifecycle_deliveries_builds_rows_for_matching_active_subs() {
        // A non-default tenant: this is the exact call shape `create_link_core`/
        // `admin_link_delete`/`admin_link_patch` use, so the row must be
        // scoped to (and stamped with) THIS tenant, not `DEFAULT_TENANT`.
        let tenant = crate::tenant::TenantId(7);
        let stub = Arc::new(StubStore::new_multi(vec![(
            tenant,
            vec![
                sub(
                    7,
                    "https://a",
                    vec![EventType::LinkCreated],
                    true,
                    "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw",
                ),
                sub(
                    8,
                    "https://b",
                    vec![EventType::LinkDeleted],
                    true,
                    "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw",
                ),
                sub(
                    9,
                    "https://c",
                    vec![EventType::LinkCreated],
                    false,
                    "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw",
                ),
            ],
        )]));
        let store: Arc<dyn Store> = stub.clone();
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let dispatcher = WebhookDispatcher::new(
            tx,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        )
        .with_outbox(store);

        let rows = dispatcher
            .lifecycle_deliveries(
                tenant,
                &WebhookEvent {
                    event_type: EventType::LinkCreated,
                    body: r#"{"id":"evt_abc","type":"link.created"}"#.to_string(),
                    tenant_id: tenant,
                },
            )
            .await;

        assert_eq!(rows.len(), 1, "only the active link.created sub matches");
        assert_eq!(rows[0].delivery_key, "evt_abc.7");
        assert_eq!(rows[0].subscription_id, 7);
        assert_eq!(
            rows[0].tenant_id, tenant,
            "the row must be stamped with the passed tenant, not DEFAULT_TENANT"
        );
        assert_eq!(
            *stub.seen_tenant.lock().unwrap(),
            Some(tenant),
            "list_webhooks must be called with the passed tenant"
        );
        // Outbox path must not emit onto the in-memory channel.
        assert!(rx.try_recv().is_err());
    }

    /// Without an outbox (LMDB), `lifecycle_deliveries` returns no rows and
    /// falls back to the in-memory `emit` (single-node behavior unchanged).
    #[tokio::test]
    async fn lmdb_lifecycle_deliveries_is_pure_and_emit_if_in_memory_emits() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let dispatcher = WebhookDispatcher::new(
            tx,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        );
        let ev = WebhookEvent {
            event_type: EventType::LinkCreated,
            body: "{}".to_string(),
            tenant_id: crate::tenant::DEFAULT_TENANT,
        };

        let rows = dispatcher
            .lifecycle_deliveries(crate::tenant::DEFAULT_TENANT, &ev)
            .await;
        assert!(rows.is_empty());
        assert!(
            rx.try_recv().is_err(),
            "lifecycle_deliveries must not emit; the emit is deferred to after the mutation"
        );

        dispatcher.emit_if_in_memory(ev);
        let got = rx.try_recv().expect("emit_if_in_memory emits on LMDB");
        assert_eq!(got.event_type, EventType::LinkCreated);
    }
}
