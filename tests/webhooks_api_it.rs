//! Admin `/admin/webhooks` CRUD, secret masking, and emission at the
//! dispatcher boundary. Mirrors the `/admin/links` test preamble in
//! `tests/api_it.rs` (`app_admin("secret")`, `ServiceExt::oneshot`,
//! `x-admin-token`).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use quark::api::router;
use quark::cache::Cache;
use quark::store::open_backends;
use quark::webhooks::delivery::WebhookDispatcher;
use quark::webhooks::{EventType, WebhookEvent};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tower::ServiceExt;

mod common;

/// Builds a router with an admin token and a `WebhookDispatcher` whose
/// channel the test can drain, to assert emission without going through
/// real HTTP delivery.
async fn app_admin_with_dispatcher(
    token: &str,
) -> (axum::Router, tokio::sync::mpsc::Receiver<WebhookEvent>) {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    let cache = Cache::new(store.clone(), 1000, None);
    let host_router = Arc::new(quark::domain_router::HostRouter::new(
        store.clone(),
        None,
        None,
    ));
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
    let (wh_tx, wh_rx) = tokio::sync::mpsc::channel(100);
    let webhooks = Arc::new(WebhookDispatcher::new(
        wh_tx,
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    ));
    let state = common::TestState::new(store, sink)
        .cache(cache)
        .host_router(host_router)
        .analytics_tx(tx)
        .webhooks(webhooks)
        .admin_token(Some(token.to_string()))
        .build();
    (router(state), wh_rx)
}

async fn app_admin(token: &str) -> axum::Router {
    app_admin_with_dispatcher(token).await.0
}

/// Same as `app_admin_with_dispatcher`, but with `clicked_subscribed` preset
/// to `true` up front, so the redirect handler's webhook gate is open
/// without waiting on the delivery worker's periodic refresh.
async fn app_admin_with_dispatcher_clicked_subscribed(
    token: &str,
) -> (axum::Router, tokio::sync::mpsc::Receiver<WebhookEvent>) {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    let cache = Cache::new(store.clone(), 1000, None);
    let host_router = Arc::new(quark::domain_router::HostRouter::new(
        store.clone(),
        None,
        None,
    ));
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
    let (wh_tx, wh_rx) = tokio::sync::mpsc::channel(100);
    let webhooks = Arc::new(WebhookDispatcher::new(
        wh_tx,
        Arc::new(AtomicBool::new(true)),
        Arc::new(AtomicBool::new(false)),
    ));
    let state = common::TestState::new(store, sink)
        .cache(cache)
        .host_router(host_router)
        .analytics_tx(tx)
        .webhooks(webhooks)
        .admin_token(Some(token.to_string()))
        .build();
    (router(state), wh_rx)
}

#[tokio::test]
async fn webhooks_post_requires_token() {
    let app = app_admin("secret").await;
    let resp = app
        .oneshot(
            Request::post("/admin/webhooks")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"url":"https://example.com/hook","events":["link.created"]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn webhooks_post_rejects_internal_url() {
    let app = app_admin("secret").await;
    let resp = app
        .oneshot(
            Request::post("/admin/webhooks")
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(
                    r#"{"url":"http://127.0.0.1:9000","events":["link.created"]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn webhooks_crud_and_secret_masking() {
    let app = app_admin("secret").await;

    let resp = app
        .clone()
        .oneshot(
            Request::post("/admin/webhooks")
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(
                    r#"{"url":"https://example.com/hook","events":["link.created","link.deleted"]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let id = created["id"].as_u64().unwrap();
    let secret = created["secret"].as_str().unwrap();
    assert!(secret.starts_with("whsec_"));

    let resp = app
        .clone()
        .oneshot(
            Request::get("/admin/webhooks")
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let list: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let row = &list["webhooks"][0];
    assert_eq!(row["id"], id);
    assert_eq!(row["url"], "https://example.com/hook");
    assert_eq!(
        row["secret_masked"],
        "whsec_\u{2022}\u{2022}\u{2022}\u{2022}"
    );
    assert!(row.get("secret").is_none());
    assert_eq!(row["kind"], "generic");

    let resp = app
        .clone()
        .oneshot(
            Request::patch(format!("/admin/webhooks/{id}"))
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(r#"{"active":false}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(
            Request::patch(format!("/admin/webhooks/{id}"))
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(r#"{"url":"http://127.0.0.1:9000"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let resp = app
        .clone()
        .oneshot(
            Request::delete(format!("/admin/webhooks/{id}"))
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(
            Request::delete(format!("/admin/webhooks/{id}"))
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn create_and_list_webhook_exposes_connector_id_and_health() {
    let app = app_admin("secret").await;

    let resp = app
        .clone()
        .oneshot(
            Request::post("/admin/webhooks")
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(
                    r#"{"url":"https://example.com/hook","events":["link.created"],"kind":"generic","connector_id":"zapier"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let id = created["id"].as_u64().unwrap();

    let resp = app
        .oneshot(
            Request::get("/admin/webhooks")
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let list: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let row = &list["webhooks"][0];
    assert_eq!(row["id"], id);
    assert_eq!(row["connector_id"], "zapier");
    assert_eq!(
        row["last_delivery_status"],
        serde_json::json!({"state": "never"})
    );
    assert!(row.get("last_delivery_at").is_none() || row["last_delivery_at"].is_null());
}

/// LUC-87 fase 3: `POST /admin/webhooks/:id/test` must record passive
/// delivery health for the tested subscription, the same as a real delivery
/// would (`webhooks::delivery::deliver_one`) — this is the only health
/// signal a `link.clicked`-only subscription can ever get, since that event
/// deliberately never touches the store on the redirect hot path. The
/// destination host is a non-resolvable, non-loopback name: it passes both
/// `validate_webhook_url` (creation) and the test endpoint's own
/// `is_internal_host` SSRF guard (which only catches literal loopback/private
/// addresses and `localhost`, never resolves DNS), so the request reaches
/// `reqwest::Client::send` and fails there with a transport error, a clean,
/// deterministic error-path proof that recording happened without needing a
/// real reachable destination.
#[tokio::test]
async fn test_endpoint_records_passive_health_on_delivery_failure() {
    let app = app_admin("secret").await;

    let resp = app
        .clone()
        .oneshot(
            Request::post("/admin/webhooks")
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(
                    r#"{"url":"http://this-host-does-not-exist.invalid/hook","events":["link.clicked"]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let id = created["id"].as_u64().unwrap();

    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/admin/webhooks/{id}/test"))
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let test_result: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(test_result["delivered"], false);

    let resp = app
        .clone()
        .oneshot(
            Request::get("/admin/webhooks")
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let list: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let row = &list["webhooks"][0];
    assert_eq!(row["id"], id);
    assert_eq!(row["last_delivery_status"]["state"], "error");
    assert!(row["last_delivery_at"].is_number());

    // LUC-87 fase 3: the transport-error detail must never leak the
    // destination URL — reqwest::Error's Display includes the request URL
    // by default, and for channel webhooks (Slack/Discord/Telegram/...) the
    // secret token lives IN the URL. Create a second subscription whose URL
    // carries a recognizable "secret" and drive the same transport-error
    // path through `/test`, then assert the persisted `detail` contains
    // neither the host nor the token.
    let resp = app
        .clone()
        .oneshot(
            Request::post("/admin/webhooks")
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(
                    r#"{"url":"http://this-host-does-not-exist.invalid/services/T/B/SUPERSECRET123","events":["link.clicked"]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let created2: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let id2 = created2["id"].as_u64().unwrap();

    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/admin/webhooks/{id2}/test"))
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(
            Request::get("/admin/webhooks")
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let list: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let row2 = list["webhooks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == id2)
        .expect("second webhook present");
    assert_eq!(row2["last_delivery_status"]["state"], "error");
    let detail2 = row2["last_delivery_status"]["detail"]
        .as_str()
        .expect("error detail is a string");
    assert!(
        !detail2.contains("SUPERSECRET123"),
        "detail leaked the webhook secret: {detail2}"
    );
    assert!(
        !detail2.contains("this-host-does-not-exist.invalid"),
        "detail leaked the destination host: {detail2}"
    );
}

#[tokio::test]
async fn creating_a_link_emits_link_created() {
    let (app, mut wh_rx) = app_admin_with_dispatcher("secret").await;

    let resp = app
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(r#"{"url":"https://example.com"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let ev = wh_rx.try_recv().expect("expected an emitted WebhookEvent");
    assert_eq!(ev.event_type, EventType::LinkCreated);
    let payload: serde_json::Value = serde_json::from_str(&ev.body).unwrap();
    assert_eq!(payload["type"], "link.created");
    assert_eq!(payload["data"]["url"], "https://example.com");
}

/// Regression for the "clicked payload incomplete" review finding: with an
/// active `link.clicked` subscription, a redirect must emit a payload whose
/// `data` carries the click context (country/referrer/device/ts) already
/// captured for analytics via the same `ClickEvent`, not just
/// `{code,url,created}`.
#[tokio::test]
async fn link_clicked_emits_payload_with_click_context() {
    let (app, mut wh_rx) = app_admin_with_dispatcher_clicked_subscribed("secret").await;

    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(r#"{"url":"https://example.com"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let code = created["code"].as_str().unwrap().to_string();
    // Drain the link.created event emitted by the POST above.
    let _ = wh_rx
        .try_recv()
        .expect("expected a link.created event from creation");

    let resp = app
        .oneshot(
            Request::get(format!("/{code}"))
                .header("referer", "https://ref.example/page")
                .header("cf-ipcountry", "BR")
                .header("user-agent", "Mozilla/5.0 (iPhone; CPU iPhone OS)")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);

    let ev = wh_rx
        .try_recv()
        .expect("expected an emitted link.clicked WebhookEvent");
    assert_eq!(ev.event_type, EventType::LinkClicked);
    let payload: serde_json::Value = serde_json::from_str(&ev.body).unwrap();
    assert_eq!(payload["type"], "link.clicked");
    assert_eq!(payload["data"]["code"], code);
    assert_eq!(payload["data"]["country"], "BR");
    assert_eq!(payload["data"]["referrer"], "https://ref.example/page");
    assert_eq!(payload["data"]["device"], "Mobile");
    assert!(payload["data"]["ts"].is_u64());
}

/// Creating a Slack-kind subscription must not mint an HMAC secret: Slack
/// authenticates via the incoming URL itself, so `secret` is absent from the
/// response entirely (rather than a fake/empty `whsec_...` value), and the
/// list's `secret_masked` is empty too.
#[tokio::test]
async fn creating_a_slack_webhook_returns_no_secret() {
    let app = app_admin("secret").await;

    let resp = app
        .clone()
        .oneshot(
            Request::post("/admin/webhooks")
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(
                    r#"{"url":"https://hooks.slack.example/incoming","events":["link.created"],"kind":"slack"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(created.get("secret").is_none());
    let id = created["id"].as_u64().unwrap();

    let resp = app
        .oneshot(
            Request::get("/admin/webhooks")
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let list: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let row = &list["webhooks"][0];
    assert_eq!(row["id"], id);
    assert_eq!(row["kind"], "slack");
    assert_eq!(row["secret_masked"], "");
}

/// PATCHing a channel subscription's `kind` back to `generic` must
/// reconcile the secret: a Slack sub has an empty secret (see
/// `creating_a_slack_webhook_returns_no_secret`), so if the patch didn't
/// mint one, the resulting Generic subscription would sign every delivery
/// with an empty HMAC key (`sign("", ...)` doesn't error) — a signature any
/// third party can reproduce. And the inverse: patching a Generic sub
/// (non-empty secret) to a channel kind must zero the secret out, since a
/// channel authenticates via its URL and a signing secret makes no sense
/// there.
#[tokio::test]
async fn patching_kind_reconciles_the_secret() {
    let app = app_admin("secret").await;

    let resp = app
        .clone()
        .oneshot(
            Request::post("/admin/webhooks")
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(
                    r#"{"url":"https://hooks.slack.example/incoming","events":["link.created"],"kind":"slack"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let slack_id = created["id"].as_u64().unwrap();

    let resp = app
        .clone()
        .oneshot(
            Request::patch(format!("/admin/webhooks/{slack_id}"))
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(r#"{"kind":"generic"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(
            Request::get("/admin/webhooks")
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let list: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let row = list["webhooks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == slack_id)
        .unwrap();
    assert_eq!(row["kind"], "generic");
    assert_eq!(
        row["secret_masked"], "whsec_\u{2022}\u{2022}\u{2022}\u{2022}",
        "kind patched slack -> generic must mint a fresh, non-empty signing secret"
    );

    // Inverse: create a Generic sub (non-empty secret), then patch it to a
    // channel kind; the secret must be zeroed.
    let resp = app
        .clone()
        .oneshot(
            Request::post("/admin/webhooks")
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(
                    r#"{"url":"https://example.com/hook","events":["link.created"]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(created["secret"].as_str().unwrap().starts_with("whsec_"));
    let generic_id = created["id"].as_u64().unwrap();

    let resp = app
        .clone()
        .oneshot(
            Request::patch(format!("/admin/webhooks/{generic_id}"))
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(r#"{"kind":"slack"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(
            Request::get("/admin/webhooks")
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let list: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let row = list["webhooks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == generic_id)
        .unwrap();
    assert_eq!(row["kind"], "slack");
    assert_eq!(
        row["secret_masked"], "",
        "kind patched generic -> slack must zero out the signing secret"
    );
}

/// Hot-path coverage for the no-subscriber case: `app_admin_with_dispatcher`
/// starts `clicked_subscribed` at `false` (no active `link.clicked`
/// subscription), so a redirect must not enqueue a `WebhookEvent` at all.
/// This is the gate in `api::redirect` (`st.webhooks.clicked_subscribed.load`)
/// that keeps the hot path from paying for a payload build when nobody is
/// listening; regressing it back to always-emit wouldn't fail any other test
/// in this file, since the other click test presets the flag to `true`.
#[tokio::test]
async fn link_clicked_does_not_emit_without_subscriber() {
    let (app, mut wh_rx) = app_admin_with_dispatcher("secret").await;

    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(r#"{"url":"https://example.com"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let code = created["code"].as_str().unwrap().to_string();
    // Drain the link.created event emitted by the POST above.
    let _ = wh_rx
        .try_recv()
        .expect("expected a link.created event from creation");

    // Keep `app` alive (clone rather than consume) so the `AppState`'s
    // `WebhookDispatcher` sender is still held open when we check the
    // channel below; otherwise dropping the last `Router` closes the
    // channel and `try_recv` reports `Disconnected` instead of `Empty`,
    // even though the real signal (no message was queued) is the same.
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/{code}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);

    assert!(matches!(
        wh_rx.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
}
