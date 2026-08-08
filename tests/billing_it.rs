// Codigo de teste pode entrar em panico: a falha e o proprio sinal.
#![allow(clippy::unwrap_used)]
// Enterprise suite: billing only exists in the `--features ee` build (LUC-41).
#![cfg(feature = "ee")]

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
