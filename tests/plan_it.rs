// Codigo de teste pode entrar em panico: a falha e o proprio sinal.
#![allow(clippy::unwrap_used)]
// Enterprise suite: plans only exist in the `--features ee` build (LUC-41).
#![cfg(feature = "ee")]

use quark::analytics::AnalyticsSink;
use quark::api::entitlement::{require, require_quota, Feature, Quota};
use quark::store::postgres::PostgresStore;
use quark::store::Store;
use quark::tenant::{Tenant, TenantId, DEFAULT_TENANT};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

/// Break-glass (`x-admin-token` against `st.admin_token`) always resolves to
/// `DEFAULT_TENANT`, never to a tenant created in test setup (see
/// `admin_guard`). An HTTP-level 402 test that authenticates via break-glass
/// therefore has to grant the plan on `DEFAULT_TENANT`, not on some other
/// tenant id, or it would be asserting on a plan nobody is actually reading.
const ADMIN_TOKEN: &str = "test-admin-token";

async fn state_with_plan_on_default_tenant(plan: &str) -> std::sync::Arc<quark::api::AppState> {
    let url = std::env::var("QUARK_TEST_DATABASE_URL").unwrap();
    let store = Arc::new(PostgresStore::open(&url, true).await.unwrap());
    store.reset_for_tests().await.unwrap();
    store
        .put_tenant(&Tenant {
            id: DEFAULT_TENANT,
            name: "Default".into(),
            slug: "default".into(),
            created: 0,
        })
        .await
        .unwrap();
    store.set_tenant_plan(DEFAULT_TENANT, plan).await.unwrap();
    let sink: Arc<dyn AnalyticsSink> = store.clone();
    common::TestState::new(store.clone(), sink)
        .admin_token(Some(ADMIN_TOKEN.into()))
        .build()
}

async fn state_with_plan(plan: &str) -> (std::sync::Arc<quark::api::AppState>, TenantId) {
    let url = std::env::var("QUARK_TEST_DATABASE_URL").unwrap();
    let store = Arc::new(PostgresStore::open(&url, true).await.unwrap());
    store.reset_for_tests().await.unwrap();
    let t = TenantId(7001);
    store
        .put_tenant(&Tenant {
            id: t,
            name: "Acme".into(),
            slug: "acme-ent".into(),
            created: 0,
        })
        .await
        .unwrap();
    store.set_tenant_plan(t, plan).await.unwrap();
    let sink: Arc<dyn AnalyticsSink> = store.clone();
    let st = common::TestState::new(store.clone(), sink)
        .multi_tenant(true)
        .build();
    (st, t)
}

#[tokio::test]
#[serial_test::file_serial]
async fn free_is_denied_webhooks_and_told_where_to_go() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    let (st, t) = state_with_plan("free").await;
    let denied = require(&st, t, Feature::Webhooks).await.unwrap_err();
    assert_eq!(denied.limit, "webhooks");
    assert_eq!(denied.allowed, None);
    assert_eq!(denied.upgrade_to, "starter");
}

#[tokio::test]
#[serial_test::file_serial]
async fn starter_is_allowed_webhooks() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    let (st, t) = state_with_plan("starter").await;
    assert!(require(&st, t, Feature::Webhooks).await.is_ok());
}

#[tokio::test]
#[serial_test::file_serial]
async fn domain_quota_allows_at_the_ceiling_and_denies_above_it() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    let (st, t) = state_with_plan("free").await;
    // Free allows 3 domains: holding 2 is fine, holding 3 is not.
    assert!(require_quota(&st, t, Quota::Domains, 2).await.is_ok());
    let denied = require_quota(&st, t, Quota::Domains, 3).await.unwrap_err();
    assert_eq!(denied.limit, "domains");
    assert_eq!(denied.allowed, Some(3));
    assert_eq!(denied.upgrade_to, "starter");
}

#[tokio::test]
#[serial_test::file_serial]
async fn business_has_no_domain_ceiling() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    let (st, t) = state_with_plan("business").await;
    assert!(require_quota(&st, t, Quota::Domains, 10_000).await.is_ok());
}

/// End-to-end proof that the HTTP handler, not just `require` in isolation,
/// is wired to the gate: a Free-plan tenant creating a webhook through the
/// real admin route gets `402`, not `201`.
#[tokio::test]
#[serial_test::file_serial]
async fn free_tenant_gets_402_creating_a_webhook() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    let st = state_with_plan_on_default_tenant("free").await;
    let app = quark::api::router(st.clone());
    let body = serde_json::json!({
        "url": "https://example.com/hook",
        "events": ["link.created"]
    });
    let res = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/admin/webhooks")
                .header("x-admin-token", ADMIN_TOKEN)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::PAYMENT_REQUIRED);
}
