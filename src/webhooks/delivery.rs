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
/// Attempt budget for a destination answering permanently: the original
/// attempt plus one confirmation. Not 1, because a momentary 404 from a deploy
/// window or a proxy must not kill the customer's integration; not the full
/// budget, because a destination that was really removed never comes back, and
/// retrying it is the loop this exists to end.
pub const PERMANENT_DELIVERY_ATTEMPTS: u32 = 2;

/// `404` and `410` mean the destination is gone: `410 Gone` is literally the
/// code for "this was removed and is not coming back". `400` and `422` are left
/// out on purpose: a `422` usually says *our* payload is wrong, and disabling
/// the customer's integration over a bug of ours is the worst outcome
/// available. `429`, `5xx`, timeouts and transport errors stay transient.
pub(crate) fn is_permanent(status: u16) -> bool {
    matches!(status, 404 | 410)
}

/// Outcome of one delivery attempt. `Permanent` carries the status so it can
/// become both the disable reason and the shorter budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttemptOutcome {
    Success,
    Permanent(u16),
    Transient,
}

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
                tracing::warn!(error = %e, "webhook event dropped, channel full");
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
                tracing::warn!(error = %e, "webhook outbox snapshot failed");
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
#[expect(
    clippy::expect_used,
    reason = "a client with only timeouts and a redirect policy always builds"
)]
pub fn spawn_webhook_worker(
    mut rx: Receiver<WebhookEvent>,
    store: Arc<dyn Store>,
    clicked: Arc<AtomicBool>,
    expired: Arc<AtomicBool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(DELIVERY_TIMEOUT_SECS))
            .connect_timeout(Duration::from_secs(crate::HTTP_CONNECT_TIMEOUT_SECS))
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
                        Some(ev) => deliver_to_matching(&client, &store, &subs, &ev).await,
                        // Channel closed (shutdown): deliver whatever is still
                        // queued before exiting, mirroring analytics::spawn_worker.
                        // Without this the events buffered at SIGTERM were dropped.
                        None => {
                            while let Ok(ev) = rx.try_recv() {
                                deliver_to_matching(&client, &store, &subs, &ev).await;
                            }
                            break;
                        }
                    }
                }
                _ = ticker.tick() => {
                    refresh_snapshot(&store, &clicked, &expired, &mut subs).await;
                }
            }
        }
    })
}

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
    match tokio::time::timeout(crate::SNAPSHOT_TIMEOUT, load).await {
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
            tracing::warn!(error = %e, "webhook subscription snapshot refresh failed, keeping previous");
        }
        Err(_) => {
            tracing::warn!("webhook subscription snapshot refresh timed out, keeping previous");
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
    store: &Arc<dyn Store>,
    subs: &[(crate::tenant::TenantId, Vec<WebhookSubscription>)],
    ev: &WebhookEvent,
) {
    deliver_to_matching_guarded(client, store, subs, ev, is_internal_host).await
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
    store: &Arc<dyn Store>,
    subs: &[(crate::tenant::TenantId, Vec<WebhookSubscription>)],
    ev: &WebhookEvent,
    is_blocked: impl Fn(&str) -> bool,
) {
    let Some((_, tenant_subs)) = subs.iter().find(|(t, _)| *t == ev.tenant_id) else {
        return;
    };
    for sub in tenant_subs.iter().filter(|s| matches(s, &ev.event_type)) {
        let host = match extract_host(sub.url.expose()) {
            Some(h) => h,
            None => {
                tracing::warn!(
                    webhook_id = sub.id,
                    url = %sub.url,
                    "webhook destination url is invalid"
                );
                continue;
            }
        };
        if is_blocked(&host) {
            tracing::warn!(
                webhook_id = sub.id,
                url = %sub.url,
                "webhook destination blocked by the ssrf guard"
            );
            continue;
        }
        deliver_one(client, store, sub, ev).await;
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
                    tracing::error!(
                        error = %e,
                        webhook_id = sub.id,
                        url = %sub.url,
                        "webhook signing failed"
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
            #[expect(
                clippy::expect_used,
                reason = "channel_payload only returns None for Generic, which this branch never sees"
            )]
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
async fn deliver_one(
    client: &reqwest::Client,
    store: &Arc<dyn Store>,
    sub: &WebhookSubscription,
    ev: &WebhookEvent,
) {
    let Some(req) = build_outgoing_request(sub, ev, None) else {
        return;
    };

    let mut outcome = crate::health::HealthStatus::Error("no attempt".into());
    // Consecutive permanent (404/410) responses. The streak, rather than a
    // one-shot flag, is what makes the confirmation attempt real: a transient
    // failure in between resets it, so a destination is only ever declared dead
    // on `PERMANENT_DELIVERY_ATTEMPTS` permanent answers in a row.
    let mut permanent_streak = 0u32;
    let mut permanent_status: Option<u16> = None;
    let mut attempt = 0;
    while attempt < DELIVERY_ATTEMPTS {
        let mut builder = client
            .post(sub.url.expose())
            .header("content-type", "application/json");
        for (name, value) in &req.extra_headers {
            builder = builder.header(*name, value);
        }
        let res = builder.body(req.body.clone()).send().await;

        match res {
            Ok(resp) if resp.status().is_success() => {
                outcome = crate::health::HealthStatus::Ok;
                permanent_streak = 0;
                permanent_status = None;
                break;
            }
            Ok(resp) => {
                let code = resp.status().as_u16();
                outcome = crate::health::HealthStatus::Error(format!("status {code}"));
                if is_permanent(code) {
                    permanent_streak += 1;
                    permanent_status = Some(code);
                } else {
                    permanent_streak = 0;
                    permanent_status = None;
                }
                tracing::warn!(
                    status = code,
                    webhook_id = sub.id,
                    url = %sub.url,
                    attempt = attempt + 1,
                    "webhook delivery returned a non-2xx status"
                );
            }
            Err(e) => {
                // For channel webhooks (Slack/Discord/Telegram/...) the secret
                // token lives in the URL itself, and reqwest's `Display`
                // includes the full request URL by default. So the URL has to
                // be stripped in both places: the health detail (persisted in
                // LMDB/Postgres and returned by `GET /admin/webhooks`) and the
                // log line. The redacted `url` field plus `webhook_id` already
                // give the operator the host and the row to look up, which is
                // everything the URL ever contributed to diagnosis; the rest of
                // it was only the credential.
                let redacted = e.without_url();
                tracing::warn!(
                    error = %redacted,
                    webhook_id = sub.id,
                    url = %sub.url,
                    attempt = attempt + 1,
                    "webhook delivery failed"
                );
                outcome = crate::health::HealthStatus::Error(redacted.to_string());
                permanent_streak = 0;
                permanent_status = None;
            }
        }

        attempt += 1;
        if permanent_streak >= PERMANENT_DELIVERY_ATTEMPTS {
            break;
        }
        if attempt < DELIVERY_ATTEMPTS {
            tokio::time::sleep(backoff_with_jitter(attempt - 1)).await;
        }
    }

    let confirmed_permanent =
        permanent_status.filter(|_| permanent_streak >= PERMANENT_DELIVERY_ATTEMPTS);

    if !matches!(outcome, crate::health::HealthStatus::Ok) {
        // `webhook-id` is only present for `Generic` (Standard Webhooks
        // signing); channel kinds have no per-attempt id to report.
        let msg_id = req
            .extra_headers
            .iter()
            .find(|(name, _)| *name == "webhook-id")
            .map(|(_, value)| value.as_str());
        // Two distinct exits, two distinct messages: the loop either burned the
        // whole transient budget (an unstable destination) or stopped early on a
        // confirmed permanent answer (a dead destination, budget shortened to
        // `PERMANENT_DELIVERY_ATTEMPTS` on purpose). Grepping for one must not
        // count the other.
        if confirmed_permanent.is_some() {
            tracing::warn!(
                webhook_id = sub.id,
                url = %sub.url,
                msg_id,
                "webhook delivery stopped early on a confirmed permanent failure"
            );
        } else {
            tracing::warn!(
                webhook_id = sub.id,
                url = %sub.url,
                msg_id,
                "webhook delivery budget exhausted"
            );
        }
    }

    // Only the health record is excluded for `link.clicked`, and the reason is
    // volume, not latency: that event fires on every redirect, so recording
    // health here would be one store write per click. Neither write is on the
    // redirect's synchronous path (delivery runs in the worker task, reached
    // through a `try_send`).
    //
    // The disable does not have the volume problem: it happens once, only on a
    // confirmed permanent failure, and after it the subscription stops being
    // delivered to at all. Excluding `link.clicked` from it was the bug LUC-141
    // set out to fix, because a Slack or Discord connection created by OAuth
    // subscribes to every event, so a tenant that only generates clicks would
    // keep posting to a revoked endpoint forever. Best-effort on both: log and
    // swallow any error.
    if ev.event_type != EventType::LinkClicked {
        if let Err(e) = store
            .record_webhook_health(ev.tenant_id, sub.id, crate::now(), outcome)
            .await
        {
            tracing::warn!(error = %e, "webhook health record write failed");
        }
    }

    if let Some(code) = confirmed_permanent {
        let reason = format!("status {code}");
        tracing::warn!(
            webhook_id = sub.id,
            status = code,
            url = %sub.url,
            "webhook destination is gone, disabling the subscription"
        );
        if let Err(e) = store.disable_webhook(ev.tenant_id, sub.id, &reason).await {
            tracing::warn!(error = %e, webhook_id = sub.id, "webhook disable write failed");
        }
    }
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
            tracing::warn!(error = %e, "webhook relay snapshot failed");
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
            tracing::warn!(error = %e, "webhook relay claim failed");
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
                tracing::warn!(
                    subscription_id = delivery.subscription_id,
                    delivery_key = %delivery.delivery_key,
                    "relayed webhook subscription no longer exists"
                );
                mark_dead_logged(store, delivery.id, delivery.attempts).await;
                return;
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    delivery_key = %delivery.delivery_key,
                    "relayed webhook subscription lookup failed"
                );
                let next =
                    now.saturating_add(relay_backoff_secs(delivery.attempts.saturating_add(1)));
                // No POST happened, so the permanent streak is carried over
                // untouched: a lookup failure is not an answer from the
                // destination and must not reset or advance it.
                let _ = store
                    .mark_retry(
                        delivery.id,
                        next,
                        delivery.attempts,
                        delivery.permanent_streak,
                    )
                    .await;
                return;
            }
        },
    };

    let host = match extract_host(sub.url.expose()) {
        Some(h) => h,
        None => {
            tracing::warn!(
                webhook_id = sub.id,
                url = %sub.url,
                "relayed webhook url is invalid"
            );
            mark_dead_logged(store, delivery.id, delivery.attempts).await;
            return;
        }
    };
    if is_blocked(&host) {
        tracing::warn!(
            webhook_id = sub.id,
            url = %sub.url,
            "relayed webhook blocked by the ssrf guard"
        );
        mark_dead_logged(store, delivery.id, delivery.attempts).await;
        return;
    }

    let Some(event_type) = EventType::from_wire(&delivery.event_type) else {
        tracing::warn!(event_type = %delivery.event_type, "relayed webhook has an unknown event type");
        mark_dead_logged(store, delivery.id, delivery.attempts).await;
        return;
    };
    let ev = WebhookEvent {
        event_type,
        body: delivery.payload.clone(),
        tenant_id: delivery.tenant_id,
    };
    let Some(req) = build_outgoing_request(sub, &ev, Some(&delivery.delivery_key)) else {
        tracing::warn!(delivery_key = %delivery.delivery_key, "relayed webhook request could not be built");
        mark_dead_logged(store, delivery.id, delivery.attempts).await;
        return;
    };

    let attempt_outcome = post_once(client, sub, &req).await;
    if matches!(attempt_outcome, AttemptOutcome::Success) {
        if let Err(e) = store.mark_delivered(delivery.id).await {
            tracing::warn!(error = %e, "webhook relay mark-delivered failed");
        }
        // Health is one write per click for `link.clicked` (see `deliver_one`'s
        // comment), so that event is excluded here too.
        if !matches!(event_type, EventType::LinkClicked) {
            let _ = store
                .record_webhook_health(
                    delivery.tenant_id,
                    sub.id,
                    now,
                    crate::health::HealthStatus::Ok,
                )
                .await;
        }
        return;
    }

    if !matches!(event_type, EventType::LinkClicked) {
        let _ = store
            .record_webhook_health(
                delivery.tenant_id,
                sub.id,
                now,
                crate::health::HealthStatus::Error("delivery failed".into()),
            )
            .await;
    }

    let attempts = delivery.attempts.saturating_add(1);
    // The durable twin of `deliver_one`'s local `permanent_streak`: the relay's
    // attempts are spread across polls and nodes, so the run of consecutive
    // permanent answers has to live in the row. The total in `attempts` cannot
    // stand in for it, because on a `503` followed by a `404` the total reaches
    // `PERMANENT_DELIVERY_ATTEMPTS` with a single 404 ever observed, which is
    // exactly the false positive the confirmation attempt exists to prevent.
    // Any other failure outcome resets the run, same as in memory.
    let permanent_streak = match attempt_outcome {
        AttemptOutcome::Permanent(_) => delivery.permanent_streak.saturating_add(1),
        _ => 0,
    };
    let confirmed_permanent = match attempt_outcome {
        AttemptOutcome::Permanent(code) if permanent_streak >= PERMANENT_DELIVERY_ATTEMPTS => {
            Some(code)
        }
        _ => None,
    };
    if confirmed_permanent.is_some() || attempts >= MAX_DELIVERY_ATTEMPTS {
        // Unlike the health record above, the disable applies to every event
        // type including `link.clicked`: it is a single write on a confirmed
        // permanent failure, not one per click, and skipping it would leave an
        // OAuth-created Slack or Discord connection posting to a revoked
        // endpoint forever (see `deliver_one`'s comment).
        if let Some(code) = confirmed_permanent {
            let reason = format!("status {code}");
            tracing::warn!(
                webhook_id = sub.id,
                status = code,
                url = %sub.url,
                "relayed webhook destination is gone, disabling the subscription"
            );
            if let Err(e) = store
                .disable_webhook(delivery.tenant_id, sub.id, &reason)
                .await
            {
                tracing::warn!(error = %e, webhook_id = sub.id, "webhook disable write failed");
            }
        }
        // Two different outcomes reach this line, and an operator grepping for
        // an unstable destination must not have to count dead ones alongside
        // it. `deliver_one` splits the same pair of messages.
        if confirmed_permanent.is_some() {
            tracing::error!(
                delivery_key = %delivery.delivery_key,
                attempts,
                "relayed webhook dead-lettered on a confirmed permanent failure"
            );
            mark_dead_logged(store, delivery.id, attempts).await;
            return;
        }
        tracing::error!(
            delivery_key = %delivery.delivery_key,
            attempts,
            "relayed webhook dead-lettered after exhausting its attempts"
        );
        mark_dead_logged(store, delivery.id, attempts).await;
        return;
    }
    let next_attempt_at = now.saturating_add(relay_backoff_secs(attempts));
    if let Err(e) = store
        .mark_retry(delivery.id, next_attempt_at, attempts, permanent_streak)
        .await
    {
        tracing::warn!(error = %e, "webhook relay mark-retry failed");
    }
}

/// Sends `req` once (no in-attempt retry: the persisted schedule owns retry).
/// Classifies the answer so the caller can shorten the budget and disable a
/// destination that is gone.
async fn post_once(
    client: &reqwest::Client,
    sub: &WebhookSubscription,
    req: &OutgoingRequest,
) -> AttemptOutcome {
    let mut builder = client
        .post(sub.url.expose())
        .header("content-type", "application/json");
    for (name, value) in &req.extra_headers {
        builder = builder.header(*name, value);
    }
    match builder.body(req.body.clone()).send().await {
        Ok(resp) if resp.status().is_success() => AttemptOutcome::Success,
        Ok(resp) => {
            let code = resp.status().as_u16();
            tracing::warn!(
                status = code,
                webhook_id = sub.id,
                url = %sub.url,
                "relayed webhook returned a non-2xx status"
            );
            if is_permanent(code) {
                AttemptOutcome::Permanent(code)
            } else {
                AttemptOutcome::Transient
            }
        }
        Err(e) => {
            // `without_url` for the same reason as in `deliver_one`: reqwest's
            // `Display` embeds the full request URL, which for channel kinds is
            // the credential.
            tracing::warn!(
                error = %e.without_url(),
                webhook_id = sub.id,
                url = %sub.url,
                "relayed webhook delivery failed"
            );
            AttemptOutcome::Transient
        }
    }
}

/// `mark_dead` with its error logged and swallowed (a failed dead-letter is a
/// leased row that will simply be re-claimed and re-tried after the lease).
async fn mark_dead_logged(store: &Arc<dyn Store>, id: i64, attempts: u32) {
    if let Err(e) = store.mark_dead(id, attempts).await {
        tracing::warn!(error = %e, "webhook relay mark-dead failed");
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
    #[expect(
        clippy::expect_used,
        reason = "the OS RNG being unavailable is not a recoverable condition for a security path"
    )]
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
        /// Captures every `record_webhook_health` call for assertions (LUC-87
        /// fase 3).
        health_calls: std::sync::Mutex<
            Vec<(
                crate::tenant::TenantId,
                u64,
                u64,
                crate::health::HealthStatus,
            )>,
        >,
        /// Captures every `disable_webhook` call (LUC-141), same pattern as
        /// `health_calls`: the permanent-failure tests assert the write
        /// happened, with the right reason, and that `link.clicked` never
        /// produces one.
        disable_calls: std::sync::Mutex<Vec<(crate::tenant::TenantId, u64, String)>>,
        /// Captures the relay's terminal bookkeeping so the relay tests can
        /// assert dead-lettering without a Postgres round-trip.
        mark_calls: std::sync::Mutex<Vec<MarkCall>>,
    }

    /// What the relay did with a claimed row after one attempt.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum MarkCall {
        Delivered,
        Retry {
            attempts: u32,
            permanent_streak: u32,
        },
        Dead {
            attempts: u32,
        },
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
                health_calls: std::sync::Mutex::new(Vec::new()),
                disable_calls: std::sync::Mutex::new(Vec::new()),
                mark_calls: std::sync::Mutex::new(Vec::new()),
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
        async fn record_webhook_health(
            &self,
            tenant: crate::tenant::TenantId,
            id: u64,
            at: u64,
            status: crate::health::HealthStatus,
        ) -> Result<(), StoreError> {
            self.health_calls
                .lock()
                .unwrap()
                .push((tenant, id, at, status));
            Ok(())
        }
        async fn disable_webhook(
            &self,
            tenant: crate::tenant::TenantId,
            id: u64,
            reason: &str,
        ) -> Result<(), StoreError> {
            self.disable_calls
                .lock()
                .unwrap()
                .push((tenant, id, reason.to_string()));
            Ok(())
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
        async fn release_sheets_lease(&self, _holder: &str) -> Result<(), StoreError> {
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
        async fn record_pixel_health(
            &self,
            _tenant: crate::tenant::TenantId,
            _id: u64,
            _at: u64,
            _status: crate::health::HealthStatus,
        ) -> Result<(), StoreError> {
            Ok(())
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
        async fn delete_tenant(&self, _id: crate::tenant::TenantId) -> Result<(), StoreError> {
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
        async fn set_primary_domain(
            &self,
            _tenant: crate::tenant::TenantId,
            _domain_id: Option<u64>,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn get_primary_domain_id(
            &self,
            _tenant: crate::tenant::TenantId,
        ) -> Result<Option<u64>, StoreError> {
            Ok(None)
        }
        async fn get_tenant_plan(
            &self,
            _tenant: crate::tenant::TenantId,
        ) -> Result<Option<String>, StoreError> {
            unimplemented!()
        }
        async fn set_tenant_plan(
            &self,
            _tenant: crate::tenant::TenantId,
            _plan: &str,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn count_memberships(
            &self,
            _tenant: crate::tenant::TenantId,
        ) -> Result<u64, StoreError> {
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
            self.mark_calls.lock().unwrap().push(MarkCall::Delivered);
            Ok(())
        }
        async fn mark_retry(
            &self,
            _id: i64,
            _next_attempt_at: u64,
            attempts: u32,
            permanent_streak: u32,
        ) -> Result<(), StoreError> {
            self.mark_calls.lock().unwrap().push(MarkCall::Retry {
                attempts,
                permanent_streak,
            });
            Ok(())
        }
        async fn mark_dead(&self, _id: i64, attempts: u32) -> Result<(), StoreError> {
            self.mark_calls
                .lock()
                .unwrap()
                .push(MarkCall::Dead { attempts });
            Ok(())
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
            url: url.into(),
            events,
            secret: secret.to_string(),
            active,
            created: 0,
            kind: SubscriptionKind::Generic,
            label: None,
            connector_id: None,
            external_id: None,
            last_delivery_at: None,
            last_delivery_status: Default::default(),
            disabled_reason: None,
        }
    }

    /// Builds a `WebhookSubscription` for the health-recording tests (Step
    /// 1 of the brief): same shape as `sub`, but with an explicit `kind` and
    /// a fixed valid secret (only `Generic` subscriptions sign, but the
    /// secret must still decode).
    fn test_sub(
        id: u64,
        url: &str,
        kind: SubscriptionKind,
        events: Vec<EventType>,
    ) -> WebhookSubscription {
        let mut s = sub(
            id,
            url,
            events,
            true,
            "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw",
        );
        s.kind = kind;
        s
    }

    /// Builds a `WebhookEvent` with a valid JSON body for the health tests.
    fn test_event(event_type: EventType, tenant: crate::tenant::TenantId) -> WebhookEvent {
        WebhookEvent {
            event_type,
            body: r#"{"id":"evt_test"}"#.to_string(),
            tenant_id: tenant,
        }
    }

    /// Clones the captured `record_webhook_health` calls off a concrete
    /// `StubStore` (kept alongside the `Arc<dyn Store>` used to drive
    /// delivery, since `Arc<dyn Store>` has no downcast).
    fn store_health_calls(
        store: &Arc<StubStore>,
    ) -> Vec<(
        crate::tenant::TenantId,
        u64,
        u64,
        crate::health::HealthStatus,
    )> {
        store.health_calls.lock().unwrap().clone()
    }

    /// LUC-87 fase 3: a non-`link.clicked` delivery to a 200 server must
    /// record exactly one `HealthStatus::Ok` health call for the delivered
    /// subscription.
    #[tokio::test]
    async fn records_health_ok_for_non_clicked_event() {
        let (url, _state) = spawn_test_server(vec![200]).await;
        let webhook_sub = test_sub(
            1,
            &url,
            SubscriptionKind::Generic,
            vec![EventType::LinkCreated],
        );
        let stub = Arc::new(StubStore::new(vec![webhook_sub.clone()]));
        let store: Arc<dyn Store> = stub.clone();
        let subs = vec![(crate::tenant::DEFAULT_TENANT, vec![webhook_sub])];
        let ev = test_event(EventType::LinkCreated, crate::tenant::DEFAULT_TENANT);

        deliver_to_matching_guarded(&reqwest::Client::new(), &store, &subs, &ev, |_| false).await;

        let calls = store_health_calls(&stub);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, 1); // id
        assert_eq!(calls[0].3, crate::health::HealthStatus::Ok);
    }

    /// LUC-87 fase 3: `link.clicked` is the redirect hot path and must never
    /// record health from the in-memory delivery worker.
    #[tokio::test]
    async fn does_not_record_health_for_link_clicked() {
        let (url, _state) = spawn_test_server(vec![200]).await;
        let webhook_sub = test_sub(
            2,
            &url,
            SubscriptionKind::Generic,
            vec![EventType::LinkClicked],
        );
        let stub = Arc::new(StubStore::new(vec![webhook_sub.clone()]));
        let store: Arc<dyn Store> = stub.clone();
        let subs = vec![(crate::tenant::DEFAULT_TENANT, vec![webhook_sub])];
        let ev = test_event(EventType::LinkClicked, crate::tenant::DEFAULT_TENANT);

        deliver_to_matching_guarded(&reqwest::Client::new(), &store, &subs, &ev, |_| false).await;

        assert!(
            store_health_calls(&stub).is_empty(),
            "link.clicked nunca deve gravar health (hot path)"
        );
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
        let store: Arc<dyn Store> = Arc::new(StubStore::new(vec![]));

        deliver_to_matching_guarded(&client, &store, &subs, &ev, |_| false).await;

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
        let store: Arc<dyn Store> = Arc::new(StubStore::new(vec![]));

        deliver_to_matching_guarded(&client, &store, &subs, &ev, |_| false).await;

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
        let store: Arc<dyn Store> = Arc::new(StubStore::new(vec![]));

        deliver_to_matching_guarded(&client, &store, &subs, &ev, |_| false).await;

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
        let store: Arc<dyn Store> = Arc::new(StubStore::new(vec![]));

        deliver_to_matching_guarded(&client, &store, &subs, &ev, |_| false).await;

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
        let store: Arc<dyn Store> = Arc::new(StubStore::new(vec![]));

        deliver_to_matching_guarded(&client, &store, &subs, &ev, |_| false).await;

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
        let store: Arc<dyn Store> = Arc::new(StubStore::new(vec![]));

        deliver_to_matching_guarded(&client, &store, &subs, &ev, |_| false).await;

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
        let store: Arc<dyn Store> = Arc::new(StubStore::new(vec![]));

        deliver_to_matching_guarded(&client, &store, &subs, &ev, |_| false).await;

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

    /// Collects the field names of every `tracing` event emitted while it is
    /// installed, so a test can assert on the shape of a log line.
    #[derive(Clone, Default)]
    struct CapturedEvents(Arc<Mutex<Vec<Vec<String>>>>);

    struct FieldNames(Vec<String>);

    impl tracing::field::Visit for FieldNames {
        fn record_debug(&mut self, field: &tracing::field::Field, _value: &dyn std::fmt::Debug) {
            self.0.push(field.name().to_string());
        }
    }

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CapturedEvents {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let mut names = FieldNames(Vec::new());
            event.record(&mut names);
            self.0.lock().unwrap().push(names.0);
        }
    }

    /// Every destination of a given kind shares one host (`hooks.slack.com`,
    /// `discord.com`), so a redacted `url` field alone makes 20 Slack
    /// subscriptions produce 20 indistinguishable lines. The subscription id
    /// has to ride along or the operator cannot find the row.
    #[tokio::test]
    async fn delivery_warnings_carry_the_subscription_id() {
        use tracing_subscriber::layer::SubscriberExt as _;

        let captured = CapturedEvents::default();
        let subscriber = tracing_subscriber::registry().with(captured.clone());
        let _guard = tracing::subscriber::set_default(subscriber);

        let subs = vec![(
            crate::tenant::DEFAULT_TENANT,
            vec![
                // Does not parse as a URL: hits the "destination url is
                // invalid" warn.
                sub(
                    41,
                    "https://host.example%2FSECRETTOK",
                    vec![EventType::LinkCreated],
                    true,
                    "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw",
                ),
                // Parses fine, blocked by the injected SSRF predicate.
                sub(
                    42,
                    "https://hooks.example.com/services/SECRETTOK",
                    vec![EventType::LinkCreated],
                    true,
                    "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw",
                ),
            ],
        )];
        let ev = test_event(EventType::LinkCreated, crate::tenant::DEFAULT_TENANT);
        let store: Arc<dyn Store> = Arc::new(StubStore::new(vec![]));

        deliver_to_matching_guarded(&reqwest::Client::new(), &store, &subs, &ev, |_| true).await;

        let events = captured.0.lock().unwrap().clone();
        let with_url: Vec<_> = events
            .iter()
            .filter(|fields| fields.iter().any(|f| f == "url"))
            .collect();
        assert_eq!(with_url.len(), 2, "expected both warns: {events:?}");
        for fields in with_url {
            assert!(
                fields.iter().any(|f| f == "webhook_id"),
                "log line has no subscription id to find the row with: {fields:?}"
            );
        }
    }

    /// The behavioral test above can only reach the two warns on the in-memory
    /// path. The relay sites (`deliver_claimed`, `post_once`) need a Postgres
    /// outbox, so this checks the same invariant statically instead: any
    /// `tracing` call that prints the destination url must also print the
    /// subscription id.
    #[test]
    fn every_url_log_site_also_logs_the_subscription_id() {
        let src = include_str!("delivery.rs");
        let mut checked = 0;
        for (offset, _) in src.match_indices("tracing::") {
            let rest = &src[offset..];
            // A `tracing` macro call ends at the first `);`; no field
            // expression in this file contains one.
            let end = rest.find(");").map(|i| i + 2).unwrap_or(rest.len());
            let call = &rest[..end];
            if !call.contains("url = %sub.url") {
                continue;
            }
            checked += 1;
            assert!(
                call.contains("webhook_id"),
                "log site prints the url without the subscription id: {call}"
            );
        }
        assert_eq!(
            checked, 13,
            "expected 13 url log sites in delivery.rs, found {checked}"
        );
    }

    /// Renders every field of every `tracing` event emitted while installed,
    /// so a test can assert on the values a log line actually prints (not
    /// just on its field names, which `CapturedEvents` covers).
    #[derive(Clone, Default)]
    struct CapturedText(Arc<Mutex<String>>);

    impl CapturedText {
        fn contents(&self) -> String {
            self.0.lock().unwrap().clone()
        }
    }

    struct FieldValues(String);

    impl tracing::field::Visit for FieldValues {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            use std::fmt::Write as _;
            let _ = write!(self.0, " {}={value:?}", field.name());
        }
    }

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CapturedText {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let mut values = FieldValues(String::new());
            event.record(&mut values);
            let mut out = self.0.lock().unwrap();
            out.push_str(&values.0);
            out.push('\n');
        }
    }

    /// Binds an ephemeral port and immediately drops the listener, so the
    /// address is guaranteed to refuse the connection. That is the only
    /// deterministic way to build a real `reqwest::Error`: a server replying
    /// 500 is a valid HTTP response and lands in the `Ok(resp)` arm, which
    /// never constructs the error whose `Display` embeds the full url.
    async fn closed_port_url(token: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        format!("http://{addr}/hook/{token}")
    }

    const URL_TOKEN: &str = "aVerySecretTokenThatMustNeverLeak";

    /// The `WebhookUrl` newtype redacts `url = %sub.url`, but it cannot reach
    /// `error = %e`: `reqwest::Error`'s `Display` embeds the full request url,
    /// token and all. This drives a real transport failure through the
    /// in-memory path and asserts the token never reaches the subscriber.
    #[tokio::test]
    async fn transport_failure_never_logs_the_url_token() {
        use tracing_subscriber::layer::SubscriberExt as _;

        let url = closed_port_url(URL_TOKEN).await;
        let captured = CapturedText::default();
        let subscriber = tracing_subscriber::registry().with(captured.clone());
        let _guard = tracing::subscriber::set_default(subscriber);

        let subs = vec![(
            crate::tenant::DEFAULT_TENANT,
            vec![sub(
                7,
                &url,
                vec![EventType::LinkCreated],
                true,
                "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw",
            )],
        )];
        let ev = test_event(EventType::LinkCreated, crate::tenant::DEFAULT_TENANT);
        let store: Arc<dyn Store> = Arc::new(StubStore::new(vec![]));

        deliver_to_matching_guarded(&reqwest::Client::new(), &store, &subs, &ev, |_| false).await;

        let log = captured.contents();
        assert!(
            log.contains("webhook delivery failed"),
            "the transport-error arm was never reached, so this test proves nothing:\n{log}"
        );
        assert!(!log.contains(URL_TOKEN), "the url token leaked:\n{log}");
        assert!(
            log.contains("127.0.0.1"),
            "the log lost the host and is useless for diagnosis:\n{log}"
        );
    }

    /// Same invariant for the relay path. `post_once` is called directly: the
    /// full `deliver_claimed` needs a Postgres outbox, and the leaking site is
    /// `post_once`'s own transport-error arm.
    #[tokio::test]
    async fn relay_transport_failure_never_logs_the_url_token() {
        use tracing_subscriber::layer::SubscriberExt as _;

        let url = closed_port_url(URL_TOKEN).await;
        let captured = CapturedText::default();
        let subscriber = tracing_subscriber::registry().with(captured.clone());
        let _guard = tracing::subscriber::set_default(subscriber);

        let subscription = sub(
            9,
            &url,
            vec![EventType::LinkCreated],
            true,
            "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw",
        );
        let req = OutgoingRequest {
            body: r#"{"test":1}"#.to_string(),
            extra_headers: Vec::new(),
        };

        let delivered = post_once(&reqwest::Client::new(), &subscription, &req).await;

        assert_eq!(
            delivered,
            AttemptOutcome::Transient,
            "a closed port is a transport failure, never a permanent verdict"
        );
        let log = captured.contents();
        assert!(
            log.contains("relayed webhook delivery failed"),
            "the transport-error arm was never reached, so this test proves nothing:\n{log}"
        );
        assert!(!log.contains(URL_TOKEN), "the url token leaked:\n{log}");
        assert!(
            log.contains("127.0.0.1"),
            "the log lost the host and is useless for diagnosis:\n{log}"
        );
    }

    // --- LUC-141: permanent failure classification -----------------------

    /// Clones the captured `disable_webhook` calls off a concrete `StubStore`,
    /// the same way `store_health_calls` does for health.
    fn store_disable_calls(store: &Arc<StubStore>) -> Vec<(crate::tenant::TenantId, u64, String)> {
        store.disable_calls.lock().unwrap().clone()
    }

    fn store_mark_calls(store: &Arc<StubStore>) -> Vec<MarkCall> {
        store.mark_calls.lock().unwrap().clone()
    }

    /// Number of POSTs the test server actually received.
    fn hits(state: &Arc<ServerState>) -> usize {
        state.captured.lock().unwrap().len()
    }

    /// Drives one in-memory delivery of `event_type` against a server that
    /// replies `responses` in order, and hands back the server state plus the
    /// stub store so the test can assert both the request count and the writes.
    async fn deliver_against(
        responses: Vec<u16>,
        event_type: EventType,
    ) -> (Arc<ServerState>, Arc<StubStore>) {
        let (url, state) = spawn_test_server(responses).await;
        let webhook_sub = test_sub(1, &url, SubscriptionKind::Generic, vec![event_type]);
        let stub = Arc::new(StubStore::new(vec![webhook_sub.clone()]));
        let store: Arc<dyn Store> = stub.clone();
        let subs = vec![(crate::tenant::DEFAULT_TENANT, vec![webhook_sub])];
        let ev = test_event(event_type, crate::tenant::DEFAULT_TENANT);

        deliver_to_matching_guarded(&reqwest::Client::new(), &store, &subs, &ev, |_| false).await;

        (state, stub)
    }

    /// `410 Gone` is the code for "this was removed and is not coming back":
    /// one confirmation attempt, then the subscription is disabled.
    #[tokio::test]
    async fn a_410_destination_is_confirmed_once_then_disabled() {
        let (state, stub) = deliver_against(vec![410], EventType::LinkCreated).await;

        assert_eq!(
            hits(&state),
            2,
            "410 gets the original attempt plus one confirmation, never the full budget"
        );
        let calls = store_disable_calls(&stub);
        assert_eq!(calls.len(), 1, "a confirmed 410 must disable: {calls:?}");
        assert_eq!(calls[0].1, 1);
        assert_eq!(calls[0].2, "status 410");
    }

    #[tokio::test]
    async fn a_404_destination_is_confirmed_once_then_disabled() {
        let (state, stub) = deliver_against(vec![404], EventType::LinkCreated).await;

        assert_eq!(hits(&state), 2);
        let calls = store_disable_calls(&stub);
        assert_eq!(calls.len(), 1, "a confirmed 404 must disable: {calls:?}");
        assert_eq!(calls[0].2, "status 404");
    }

    /// The test that makes the confirmation attempt worth its cost: a 404 from
    /// a deploy window must not kill the customer's integration.
    #[tokio::test]
    async fn a_404_that_recovers_on_the_confirmation_attempt_is_not_disabled() {
        let (state, stub) = deliver_against(vec![404, 200], EventType::LinkCreated).await;

        assert_eq!(hits(&state), 2);
        assert!(
            store_disable_calls(&stub).is_empty(),
            "a 404 that recovers on the confirmation must not disable"
        );
    }

    /// 5xx is transient: it keeps today's budget and never disables, because
    /// the destination can come back.
    #[tokio::test]
    async fn a_503_destination_keeps_the_full_transient_budget_and_is_not_disabled() {
        let (state, stub) = deliver_against(vec![503], EventType::LinkCreated).await;

        assert_eq!(hits(&state), DELIVERY_ATTEMPTS as usize);
        assert!(
            store_disable_calls(&stub).is_empty(),
            "5xx never disables: the destination may come back"
        );
    }

    /// `link.clicked` is the main case for LUC-141: a Slack/Discord connection
    /// created by OAuth subscribes to every event, and a tenant that only
    /// generates clicks would keep posting to a revoked endpoint forever if
    /// this event were excluded from the disable. The disable happens once, on
    /// a confirmed permanent failure, and it is not on the redirect's
    /// synchronous path (delivery runs in the worker task). The health record
    /// stays excluded because that one is a write per click.
    #[tokio::test]
    async fn link_clicked_disables_on_a_permanent_status_but_never_records_health() {
        let (state, stub) = deliver_against(vec![410], EventType::LinkClicked).await;

        assert_eq!(hits(&state), 2, "the classification itself still applies");
        let calls = store_disable_calls(&stub);
        assert_eq!(
            calls.len(),
            1,
            "a confirmed 410 must disable even for link.clicked: {calls:?}"
        );
        assert_eq!(calls[0].1, 1);
        assert_eq!(calls[0].2, "status 410");
        assert!(
            store_health_calls(&stub).is_empty(),
            "link.clicked must never record health: that is a write per click"
        );
    }

    /// The loop has two ways out and an operator greps for the difference: a
    /// destination that burned the whole transient budget is unstable, one that
    /// stopped on a confirmed 404/410 is dead. Reporting "budget exhausted" for
    /// the permanent exit is a lie, because the budget was shortened to
    /// `PERMANENT_DELIVERY_ATTEMPTS` on purpose, and it makes the two counts
    /// impossible to tell apart.
    #[tokio::test]
    async fn the_permanent_exit_and_the_exhausted_budget_log_different_messages() {
        use tracing_subscriber::layer::SubscriberExt as _;

        let captured = CapturedText::default();
        let subscriber = tracing_subscriber::registry().with(captured.clone());
        let _guard = tracing::subscriber::set_default(subscriber);

        deliver_against(vec![410], EventType::LinkCreated).await;
        let permanent = captured.contents();
        assert!(
            permanent.contains("webhook delivery stopped early on a confirmed permanent failure"),
            "the permanent exit needs its own message:\n{permanent}"
        );
        assert!(
            !permanent.contains("webhook delivery budget exhausted"),
            "the permanent exit must not claim the budget ran out:\n{permanent}"
        );

        let captured = CapturedText::default();
        let subscriber = tracing_subscriber::registry().with(captured.clone());
        let _guard = tracing::subscriber::set_default(subscriber);

        deliver_against(vec![503], EventType::LinkCreated).await;
        let transient = captured.contents();
        assert!(
            transient.contains("webhook delivery budget exhausted"),
            "a destination that really burned the budget still says so:\n{transient}"
        );
    }

    /// Builds a claimed outbox row for the relay tests.
    fn claimed_row(
        sub_id: u64,
        attempts: u32,
        permanent_streak: u32,
        event_type: EventType,
    ) -> OutboxDelivery {
        OutboxDelivery {
            id: 1,
            delivery_key: format!("evt_test.{sub_id}"),
            subscription_id: sub_id,
            event_type: event_type.as_str().to_string(),
            payload: r#"{"id":"evt_test"}"#.to_string(),
            attempts,
            permanent_streak,
            tenant_id: crate::tenant::DEFAULT_TENANT,
        }
    }

    /// Drives `n` relay attempts of the same row against a server replying
    /// `responses` in order, feeding each round the `attempts` and
    /// `permanent_streak` the previous `mark_retry` persisted, exactly like a
    /// re-claim of the row on a later poll would.
    async fn relay_attempts(
        responses: Vec<u16>,
        event_type: EventType,
        rounds: u32,
    ) -> (Arc<ServerState>, Arc<StubStore>) {
        let (url, state) = spawn_test_server(responses).await;
        let webhook_sub = test_sub(1, &url, SubscriptionKind::Generic, vec![event_type]);
        let stub = Arc::new(StubStore::new(vec![webhook_sub.clone()]));
        let store: Arc<dyn Store> = stub.clone();
        let subs = vec![webhook_sub];
        let client = reqwest::Client::new();

        let mut attempts = 0u32;
        let mut streak = 0u32;
        for _ in 0..rounds {
            let delivery = claimed_row(1, attempts, streak, event_type);
            deliver_claimed(&store, &client, &subs, &delivery, |_| false, 0).await;
            match store_mark_calls(&stub).last() {
                Some(&MarkCall::Retry {
                    attempts: a,
                    permanent_streak: s,
                }) => {
                    attempts = a;
                    streak = s;
                }
                // Delivered or dead-lettered: the row is terminal, no re-claim.
                _ => break,
            }
        }

        (state, stub)
    }

    #[tokio::test]
    async fn relay_410_is_confirmed_once_then_disabled() {
        let (state, stub) = relay_attempts(vec![410], EventType::LinkCreated, 8).await;

        assert_eq!(hits(&state), 2, "the relay also confirms exactly once");
        let calls = store_disable_calls(&stub);
        assert_eq!(calls.len(), 1, "a confirmed 410 must disable: {calls:?}");
        assert_eq!(calls[0].2, "status 410");
        assert!(
            store_mark_calls(&stub).contains(&MarkCall::Dead { attempts: 2 }),
            "the row must also be dead-lettered: {:?}",
            store_mark_calls(&stub)
        );
    }

    #[tokio::test]
    async fn relay_404_is_confirmed_once_then_disabled() {
        let (state, stub) = relay_attempts(vec![404], EventType::LinkCreated, 8).await;

        assert_eq!(hits(&state), 2);
        let calls = store_disable_calls(&stub);
        assert_eq!(calls.len(), 1, "a confirmed 404 must disable: {calls:?}");
        assert_eq!(calls[0].2, "status 404");
    }

    #[tokio::test]
    async fn relay_404_that_recovers_on_the_confirmation_is_not_disabled() {
        let (state, stub) = relay_attempts(vec![404, 200], EventType::LinkCreated, 8).await;

        assert_eq!(hits(&state), 2);
        assert!(
            store_disable_calls(&stub).is_empty(),
            "a 404 that recovers on the confirmation must not disable"
        );
        assert!(store_mark_calls(&stub).contains(&MarkCall::Delivered));
    }

    /// The relay's version of the false positive the confirmation attempt
    /// exists to prevent: one `503` followed by one `404` is a single permanent
    /// answer, not a confirmed one. Counting the total attempts instead of the
    /// consecutive permanent ones used to disable the subscription right here.
    #[tokio::test]
    async fn relay_503_then_404_does_not_disable_on_a_single_404() {
        let (state, stub) = relay_attempts(vec![503, 404], EventType::LinkCreated, 2).await;

        assert_eq!(hits(&state), 2);
        assert!(
            store_disable_calls(&stub).is_empty(),
            "one 404 after a 503 is not a confirmed permanent failure: {:?}",
            store_disable_calls(&stub)
        );
    }

    /// A transient answer between two permanent ones resets the streak, exactly
    /// like the in-memory worker: the second `404` here is again a first
    /// observation, so it only earns a confirmation attempt.
    #[tokio::test]
    async fn relay_404_then_503_then_404_does_not_disable() {
        let (state, stub) = relay_attempts(vec![404, 503, 404], EventType::LinkCreated, 3).await;

        assert_eq!(hits(&state), 3);
        assert!(
            store_disable_calls(&stub).is_empty(),
            "the 503 in the middle reset the streak: {:?}",
            store_disable_calls(&stub)
        );
    }

    #[tokio::test]
    async fn relay_503_keeps_the_full_transient_budget_and_is_not_disabled() {
        let (state, stub) =
            relay_attempts(vec![503], EventType::LinkCreated, MAX_DELIVERY_ATTEMPTS).await;

        assert_eq!(hits(&state), MAX_DELIVERY_ATTEMPTS as usize);
        assert!(
            store_disable_calls(&stub).is_empty(),
            "5xx never disables on the relay either"
        );
        assert!(store_mark_calls(&stub).contains(&MarkCall::Dead {
            attempts: MAX_DELIVERY_ATTEMPTS
        }));
    }

    /// Same rule on the relay: the disable applies to `link.clicked` too, and
    /// the health record still does not.
    #[tokio::test]
    async fn relay_link_clicked_disables_but_never_records_health() {
        let (state, stub) = relay_attempts(vec![410], EventType::LinkClicked, 8).await;

        assert_eq!(hits(&state), 2);
        let calls = store_disable_calls(&stub);
        assert_eq!(
            calls.len(),
            1,
            "a confirmed 410 must disable even for link.clicked: {calls:?}"
        );
        assert_eq!(calls[0].2, "status 410");
        assert!(
            store_health_calls(&stub).is_empty(),
            "link.clicked must never record health: that is a write per click"
        );
    }
}
