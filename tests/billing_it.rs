// Codigo de teste pode entrar em panico: a falha e o proprio sinal.
#![allow(clippy::unwrap_used)]
// Enterprise suite: billing only exists in the `--features ee` build (LUC-41).
#![cfg(feature = "ee")]
// The full-fidelity `Subscription` fixture in
// `apply_subscription_maps_status_and_lookup_key_to_the_plan` nests deep
// enough that `serde_json::json!`'s default recursion limit is not enough.
#![recursion_limit = "256"]

use quark::analytics::AnalyticsSink;
use quark::store::Store;
use quark::tenant::{Role, Tenant, TenantId};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

const PANEL: &str = "https://app.example.com";

/// State with a Postgres store, multi-tenant on, and billing configured
/// against `api_base` (a local mock, or any unreachable port for tests that
/// never get that far).
async fn state_with_billing(api_base: &str) -> (std::sync::Arc<quark::api::AppState>, TenantId) {
    let url = std::env::var("QUARK_TEST_DATABASE_URL").unwrap();
    let store = Arc::new(
        quark::store::postgres::PostgresStore::open(&url, true)
            .await
            .unwrap(),
    );
    store.reset_for_tests().await.unwrap();
    let t = TenantId(7100);
    store
        .put_tenant(&Tenant {
            id: t,
            name: "Acme".into(),
            slug: "acme-billing".into(),
            created: 0,
        })
        .await
        .unwrap();
    let sink: Arc<dyn AnalyticsSink> = store.clone();
    let billing = quark::ee::stripe::StripeBilling::from_parts(
        "sk_test_x",
        "whsec_test",
        PANEL,
        Some(api_base),
    )
    .unwrap();
    let st = common::TestState::new(store, sink)
        .multi_tenant(true)
        .oidc_configured(true)
        .billing(Some(std::sync::Arc::new(billing)))
        .build();
    (st, t)
}

/// Seeds a user, a membership with `role`, and a session cookie for it.
/// Returns the Cookie header value.
async fn seed_session(
    st: &quark::api::AppState,
    tenant: TenantId,
    user_id: u64,
    role: Role,
) -> String {
    let raw = format!("billing-session-{user_id}");
    st.store
        .put_membership(&quark::tenant::Membership {
            user_id,
            tenant_id: tenant,
            role,
            created: 0,
        })
        .await
        .unwrap();
    st.store
        .put_session(
            tenant,
            &quark::auth::Session {
                token_hash: quark::auth::hash_token(&raw),
                subject: format!("sub-{user_id}"),
                display: "user".into(),
                scopes: vec![quark::auth::Scope::Full],
                created: 0,
                // Postgres stores `expires` as a signed 64-bit column
                // (`store/postgres.rs`'s `get_session_by_hash` binds
                // `now as i64`); `u64::MAX` would wrap to -1 and the session
                // would read back as already expired. `i64::MAX` is still
                // effectively "never expires" for a test.
                expires: i64::MAX as u64,
                tenant_id: tenant,
                user_id,
                id_token: None,
            },
        )
        .await
        .unwrap();
    format!("qk_session={raw}")
}

fn post(
    uri: &str,
    cookie: Option<&str>,
    body: serde_json::Value,
) -> axum::http::Request<axum::body::Body> {
    let mut b = axum::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(c) = cookie {
        b = b.header("cookie", c);
    }
    b.body(axum::body::Body::from(body.to_string())).unwrap()
}

#[tokio::test]
#[serial_test::file_serial]
async fn checkout_is_404_when_billing_is_not_configured() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    let url = std::env::var("QUARK_TEST_DATABASE_URL").unwrap();
    let store = Arc::new(
        quark::store::postgres::PostgresStore::open(&url, true)
            .await
            .unwrap(),
    );
    store.reset_for_tests().await.unwrap();
    let sink: Arc<dyn AnalyticsSink> = store.clone();
    let st = common::TestState::new(store, sink)
        .multi_tenant(true)
        .build(); // no billing
    let app = quark::api::router(st);
    let res = app
        .oneshot(post(
            "/admin/billing/checkout",
            None,
            serde_json::json!({"plan": "pro", "cycle": "monthly", "currency": "usd"}),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
#[serial_test::file_serial]
async fn checkout_requires_a_session_and_the_owner_role() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    // Unreachable api_base: these requests must be rejected before any Stripe
    // call is attempted.
    let (st, t) = state_with_billing("http://127.0.0.1:9").await;
    let app = quark::api::router(st.clone());
    let body = serde_json::json!({"plan": "pro", "cycle": "monthly", "currency": "usd"});

    // No session: 401.
    let res = app
        .clone()
        .oneshot(post("/admin/billing/checkout", None, body.clone()))
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::UNAUTHORIZED);

    // Admin (not Owner): 403.
    let admin_cookie = seed_session(&st, t, 21, Role::Admin).await;
    let res = app
        .clone()
        .oneshot(post(
            "/admin/billing/checkout",
            Some(&admin_cookie),
            body.clone(),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::FORBIDDEN);

    // Owner with a nonsense plan: 400 before touching Stripe.
    let owner_cookie = seed_session(&st, t, 22, Role::Owner).await;
    let res = app
        .clone()
        .oneshot(post(
            "/admin/billing/checkout",
            Some(&owner_cookie),
            serde_json::json!({"plan": "free", "cycle": "monthly", "currency": "usd"}),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[serial_test::file_serial]
async fn portal_is_404_without_a_stripe_customer() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    let (st, t) = state_with_billing("http://127.0.0.1:9").await;
    let app = quark::api::router(st.clone());
    let owner_cookie = seed_session(&st, t, 23, Role::Owner).await;
    let res = app
        .oneshot(post(
            "/admin/billing/portal",
            Some(&owner_cookie),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::NOT_FOUND);
}

fn webhook_post(payload: &str, secret: &str) -> axum::http::Request<axum::body::Body> {
    let sig = stripe_webhook::Webhook::generate_test_header(payload, secret, None);
    axum::http::Request::builder()
        .method("POST")
        .uri("/stripe/webhook")
        .header("stripe-signature", sig)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(payload.to_string()))
        .unwrap()
}

/// Minimal `checkout.session.completed` event. If deserialization fails,
/// serde names the missing field: complete the fixture, do not weaken the
/// handler.
fn checkout_completed_event(event_id: &str, tenant: TenantId) -> String {
    serde_json::json!({
        "id": event_id,
        "object": "event",
        "api_version": "2026-07-29.dahlia",
        "created": 1700000000,
        "livemode": false,
        "pending_webhooks": 0,
        "type": "checkout.session.completed",
        "data": {
            "object": {
                "id": "cs_test_1",
                "object": "checkout.session",
                "mode": "subscription",
                "status": "complete",
                "client_reference_id": tenant.0.to_string(),
                "customer": "cus_123",
                "subscription": "sub_123",
                "livemode": false,
                "created": 1700000000,
                // Required (non-Option) fields on `CheckoutSession` that the
                // brief's minimal fixture omitted; miniserde rejects the
                // whole object if any of these are missing.
                "automatic_tax": {"enabled": false},
                "custom_fields": [],
                "custom_text": {},
                "expires_at": 1700003600,
                "payment_method_types": ["card"],
                "payment_status": "unpaid",
                "shipping_options": []
            }
        }
    })
    .to_string()
}

#[tokio::test]
#[serial_test::file_serial]
async fn webhook_rejects_a_bad_signature() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    let (st, t) = state_with_billing("http://127.0.0.1:9").await;
    let app = quark::api::router(st);
    let payload = checkout_completed_event("evt_sig", t);
    // Signed with the WRONG secret.
    let req = webhook_post(&payload, "whsec_wrong");
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[serial_test::file_serial]
async fn webhook_records_the_subscription_and_deduplicates() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    let (st, t) = state_with_billing("http://127.0.0.1:9").await;
    st.store.set_stripe_customer_id(t, "cus_123").await.unwrap();
    let app = quark::api::router(st.clone());
    let payload = checkout_completed_event("evt_dup", t);

    let res = app
        .clone()
        .oneshot(webhook_post(&payload, "whsec_test"))
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::OK);
    assert_eq!(
        st.store
            .get_stripe_subscription_id(t)
            .await
            .unwrap()
            .as_deref(),
        Some("sub_123")
    );

    // Same event id again: 200, no effect, and no crash.
    let res = app
        .oneshot(webhook_post(&payload, "whsec_test"))
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::OK);
}

/// Full-fidelity `Subscription` fixture: `id`/`status` are the two axes the
/// tests in this file vary, everything else is the fixed set of
/// non-`Option` fields the dahlia API requires (miniserde rejects the whole
/// object if any of these are missing).
fn subscription_fixture_json(id: &str, status: &str, tenant: TenantId) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "object": "subscription",
        "status": status,
        "customer": "cus_123",
        "cancel_at_period_end": false,
        "created": 1700000000,
        "currency": "usd",
        "livemode": false,
        "metadata": {"tenant_id": tenant.0.to_string()},
        "automatic_tax": {"enabled": false},
        "billing_cycle_anchor": 1700000000,
        "billing_mode": {"type": "classic"},
        "billing_schedules": [],
        "collection_method": "charge_automatically",
        "discounts": [],
        "invoice_settings": {"issuer": {"type": "self"}},
        "start_date": 1700000000,
        "items": {
            "object": "list",
            "url": format!("/v1/subscription_items?subscription={id}"),
            "has_more": false,
            "data": [{
                "id": "si_1",
                "object": "subscription_item",
                "created": 1700000000,
                // `current_period_end` moved from `Subscription` to
                // `SubscriptionItem` in the dahlia API (confirmed here: the
                // research doc had flagged this as unverified).
                "current_period_end": 1702592000,
                "current_period_start": 1700000000,
                "discounts": [],
                // Legacy `plan` sub-object: still required on the item even
                // though Prices superseded Plans.
                "plan": {
                    "id": "price_1",
                    "object": "plan",
                    "active": true,
                    "billing_scheme": "per_unit",
                    "created": 1700000000,
                    "currency": "usd",
                    "interval": "month",
                    "interval_count": 1,
                    "livemode": false,
                    "usage_type": "licensed"
                },
                "metadata": {},
                "quantity": 1,
                "subscription": id,
                "price": {
                    "id": "price_1",
                    "object": "price",
                    "active": true,
                    "billing_scheme": "per_unit",
                    "created": 1700000000,
                    "currency": "usd",
                    "livemode": false,
                    "lookup_key": "pro-monthly",
                    "metadata": {},
                    "product": "prod_1",
                    "type": "recurring"
                }
            }]
        }
    })
}

fn subscription_fixture(id: &str, status: &str, tenant: TenantId) -> stripe_shared::Subscription {
    serde_json::from_value(subscription_fixture_json(id, status, tenant)).unwrap()
}

/// The applier is exercised directly with a subscription deserialized from a
/// fixture: the endpoint-level path for subscription events needs a live (or
/// mocked) Stripe API for the mandatory re-fetch, which the sandbox runbook
/// covers manually.
#[tokio::test]
#[serial_test::file_serial]
async fn apply_subscription_maps_status_and_lookup_key_to_the_plan() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    let (st, t) = state_with_billing("http://127.0.0.1:9").await;
    st.store.set_stripe_customer_id(t, "cus_123").await.unwrap();

    let sub = subscription_fixture("sub_123", "active", t);
    quark::ee::api::apply_subscription(&st, &sub).await.unwrap();
    assert_eq!(
        st.store.get_tenant_plan(t).await.unwrap().as_deref(),
        Some("pro")
    );

    // Terminal status drops the tenant to free.
    let mut canceled = sub;
    canceled.status = stripe_shared::SubscriptionStatus::Canceled;
    quark::ee::api::apply_subscription(&st, &canceled)
        .await
        .unwrap();
    assert_eq!(
        st.store.get_tenant_plan(t).await.unwrap().as_deref(),
        Some("free")
    );
}

/// Cancel-and-resubscribe race: the owner cancels subscription A and
/// immediately creates B. If Stripe delivers `created`(B, paid) before
/// `deleted`(A, canceled), the stale terminal event for A must NOT clobber
/// the tenant that is now actually paying through B.
#[tokio::test]
#[serial_test::file_serial]
async fn apply_subscription_ignores_a_stale_terminal_event_for_a_superseded_subscription() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    let (st, t) = state_with_billing("http://127.0.0.1:9").await;
    st.store.set_stripe_customer_id(t, "cus_123").await.unwrap();
    // B already won: it is the tenant's current subscription, and the plan
    // is already "pro" from applying B's own created/updated event.
    st.store
        .set_stripe_subscription_id(t, "sub_B")
        .await
        .unwrap();
    st.store.set_tenant_plan(t, "pro").await.unwrap();

    // A's deleted event arrives late: terminal, but for a subscription that
    // is no longer current. Must be a no-op.
    let stale_a = subscription_fixture("sub_A", "canceled", t);
    quark::ee::api::apply_subscription(&st, &stale_a)
        .await
        .unwrap();
    assert_eq!(
        st.store.get_tenant_plan(t).await.unwrap().as_deref(),
        Some("pro"),
        "stale terminal event for a superseded subscription must not downgrade the tenant"
    );
    assert_eq!(
        st.store
            .get_stripe_subscription_id(t)
            .await
            .unwrap()
            .as_deref(),
        Some("sub_B"),
        "stale terminal event must not clobber the current subscription id"
    );

    // A terminal event for the CURRENT subscription (B itself) must still
    // downgrade normally: this guard only protects against a different,
    // already-superseded subscription.
    let terminal_b = subscription_fixture("sub_B", "canceled", t);
    quark::ee::api::apply_subscription(&st, &terminal_b)
        .await
        .unwrap();
    assert_eq!(
        st.store.get_tenant_plan(t).await.unwrap().as_deref(),
        Some("free"),
        "a terminal event for the tenant's own current subscription must still apply"
    );
}

/// Local mock Stripe server that answers `GET /v1/subscriptions/:id` with a
/// fixed JSON body, for tests that exercise `admin_billing_checkout`'s
/// live re-fetch of an existing subscription. Also answers `GET /v1/prices`
/// with a single active "pro-monthly" price, since the checkout handler
/// resolves the price by lookup key before it ever reaches the
/// subscription re-fetch.
async fn spawn_subscription_mock(body: serde_json::Value) -> String {
    async fn sub_handler(
        axum::extract::State(body): axum::extract::State<Arc<serde_json::Value>>,
    ) -> axum::Json<serde_json::Value> {
        axum::Json((*body).clone())
    }
    async fn prices_handler() -> axum::Json<serde_json::Value> {
        axum::Json(serde_json::json!({
            "object": "list",
            "url": "/v1/prices",
            "has_more": false,
            "data": [{
                "id": "price_1",
                "object": "price",
                "active": true,
                "billing_scheme": "per_unit",
                "created": 1700000000,
                "currency": "usd",
                "livemode": false,
                "lookup_key": "pro-monthly",
                "metadata": {},
                "product": "prod_1",
                "type": "recurring"
            }]
        }))
    }
    let app = axum::Router::new()
        .route("/v1/subscriptions/{id}", axum::routing::get(sub_handler))
        .route("/v1/prices", axum::routing::get(prices_handler))
        .with_state(Arc::new(body));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

/// A second checkout while a subscription is still active/trialing/past_due
/// must not open a second paid subscription for the same tenant. The
/// handler re-fetches the recorded subscription id live (the stored id
/// alone only proves a subscription once existed, not that it still pays)
/// and answers 409 when it still resolves to a paid plan.
#[tokio::test]
#[serial_test::file_serial]
async fn checkout_is_conflict_when_the_existing_subscription_is_still_active() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    // Placeholder tenant id: the mock ignores metadata/tenant matching, it
    // only needs to deserialize as a valid `Subscription`.
    let placeholder_tenant = TenantId(0);
    let sub_body = subscription_fixture_json("sub_existing", "active", placeholder_tenant);
    let api_base = spawn_subscription_mock(sub_body).await;

    let (st, t) = state_with_billing(&api_base).await;
    st.store.set_stripe_customer_id(t, "cus_123").await.unwrap();
    st.store
        .set_stripe_subscription_id(t, "sub_existing")
        .await
        .unwrap();
    let app = quark::api::router(st.clone());
    let owner_cookie = seed_session(&st, t, 24, Role::Owner).await;

    let res = app
        .oneshot(post(
            "/admin/billing/checkout",
            Some(&owner_cookie),
            serde_json::json!({"plan": "pro", "cycle": "monthly", "currency": "usd"}),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::CONFLICT);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "subscription_active");
}
