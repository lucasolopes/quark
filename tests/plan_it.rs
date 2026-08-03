// Codigo de teste pode entrar em panico: a falha e o proprio sinal.
#![allow(clippy::unwrap_used)]
// Enterprise suite: plans only exist in the `--features ee` build (LUC-41).
#![cfg(feature = "ee")]

use quark::analytics::AnalyticsSink;
use quark::api::entitlement::{require, require_quota, Feature, Quota};
use quark::ee::api::entitlement::plan_of;
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

/// `POST /admin/domains {host}` through the break-glass token, returning
/// just the status: the body isn't interesting to the callers below, only
/// whether the ceiling or the reserved-namespace check fired.
async fn create_domain(app: &axum::Router, host: &str) -> axum::http::StatusCode {
    let body = serde_json::json!({ "host": host });
    app.clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/admin/domains")
                .header("x-admin-token", ADMIN_TOKEN)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
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

/// End-to-end proof that the HTTP handler, not just `require_quota` in
/// isolation, is wired to the gate: a Free-plan tenant creating its fourth
/// custom domain through the real admin route gets `402`, not `200`. Also
/// covers the entitlement decision that the ceiling counts only
/// caller-registered domains: the tenant's automatic subdomain (the row
/// `seed_tenant_subdomain` writes at workspace creation) must not eat one of
/// the 3 slots Free allows, so all 3 caller domains below are accepted even
/// though the automatic subdomain already exists.
#[tokio::test]
#[serial_test::file_serial]
async fn free_tenant_gets_402_on_the_fourth_domain() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    let url = std::env::var("QUARK_TEST_DATABASE_URL").unwrap();
    let store = Arc::new(PostgresStore::open(&url, true).await.unwrap());
    store.reset_for_tests().await.unwrap();
    store
        .put_tenant(&Tenant {
            id: DEFAULT_TENANT,
            name: "Default".into(),
            slug: "acme".into(),
            created: 0,
        })
        .await
        .unwrap();
    store.set_tenant_plan(DEFAULT_TENANT, "free").await.unwrap();
    let sink: Arc<dyn AnalyticsSink> = store.clone();
    let suffix = "tenants.example.com";
    let st = common::TestState::new(store.clone(), sink)
        .admin_token(Some(ADMIN_TOKEN.into()))
        .multi_tenant(true)
        .tenant_domain_suffix(Some(suffix.to_string()))
        .build();

    // Seed the automatic subdomain directly, the same shape
    // `seed_tenant_subdomain` would leave behind. It must not count toward
    // the ceiling below.
    let auto_id = st.store.next_domain_id().await.unwrap();
    st.store
        .put_domain(&quark::domain::Domain {
            id: auto_id,
            tenant_id: DEFAULT_TENANT,
            host: format!("acme.{suffix}"),
            token: String::new(),
            status: quark::domain::DomainStatus::Verified,
            created: 0,
            verified_at: None,
        })
        .await
        .unwrap();

    let app = quark::api::router(st.clone());

    // All 3 of Free's slots are available to the caller, even with the
    // automatic subdomain already on file.
    for i in 0..3u64 {
        let status = create_domain(&app, &format!("d{i}.example.com")).await;
        assert_eq!(status, axum::http::StatusCode::OK);
    }
    // The 4th is denied.
    let status = create_domain(&app, "d3.example.com").await;
    assert_eq!(status, axum::http::StatusCode::PAYMENT_REQUIRED);
}

/// A caller cannot escape the domain ceiling by registering a host inside
/// our own `tenant_domain_suffix` namespace: that namespace is reserved for
/// the platform's automatic per-tenant subdomain, so any host equal to or
/// under it is rejected with `400` before it can ever be created (and
/// therefore before it could be excluded from the count as if it were the
/// automatic one). This is the fix for the bypass the first cut of the
/// exclusion logic introduced: filtering the count by suffix alone, with no
/// gate on what a caller can register, let a Free tenant create unlimited
/// `*.{suffix}` hosts that never counted against the ceiling.
#[tokio::test]
#[serial_test::file_serial]
async fn domain_under_the_tenant_suffix_is_rejected_not_silently_exempted() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    let url = std::env::var("QUARK_TEST_DATABASE_URL").unwrap();
    let store = Arc::new(PostgresStore::open(&url, true).await.unwrap());
    store.reset_for_tests().await.unwrap();
    store
        .put_tenant(&Tenant {
            id: DEFAULT_TENANT,
            name: "Default".into(),
            slug: "acme".into(),
            created: 0,
        })
        .await
        .unwrap();
    store.set_tenant_plan(DEFAULT_TENANT, "free").await.unwrap();
    let sink: Arc<dyn AnalyticsSink> = store.clone();
    let suffix = "tenants.example.com";
    let st = common::TestState::new(store.clone(), sink)
        .admin_token(Some(ADMIN_TOKEN.into()))
        .multi_tenant(true)
        .tenant_domain_suffix(Some(suffix.to_string()))
        .build();
    let app = quark::api::router(st.clone());

    // A host inside the reserved namespace is rejected, not accepted-and-
    // uncounted.
    let status = create_domain(&app, &format!("evil.{suffix}")).await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    // The suffix itself, with no subdomain, is equally reserved.
    let status = create_domain(&app, suffix).await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    // Nothing was created: the count-bypass this test guards against would
    // have left rows here even though they never counted toward the ceiling.
    assert_eq!(
        st.store.list_domains(DEFAULT_TENANT).await.unwrap().len(),
        0
    );
}

/// `GET /admin/plan` reports the grid the panel renders from: the plan
/// string, its numeric ceilings, and the features it unlocks. Authenticates
/// via break-glass, which resolves to `DEFAULT_TENANT` (see the note on
/// `ADMIN_TOKEN` above), so the plan is granted there, not on some other
/// tenant id.
#[tokio::test]
#[serial_test::file_serial]
async fn plan_endpoint_reports_the_grid_for_the_panel() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    let st = state_with_plan_on_default_tenant("starter").await;
    let app = quark::api::router(st.clone());
    let res = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/admin/plan")
                .header("x-admin-token", ADMIN_TOKEN)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["plan"], "starter");
    assert!(v["limits"]["domains"].is_number());
    assert!(v["features"].is_array());
}

/// `PUT /admin/tenants/{id}/plan` is the operator's only way to change a
/// tenant's plan: the break-glass token is required directly, the write
/// takes effect immediately (the cache is invalidated, not just left to
/// expire after the 60s TTL), and an unrecognized plan string is rejected
/// with `400` instead of silently downgrading the tenant to Free.
#[tokio::test]
#[serial_test::file_serial]
async fn operator_can_change_the_plan_and_it_takes_effect_immediately() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    let st = state_with_plan_on_default_tenant("free").await;
    let app = quark::api::router(st.clone());

    // Free is denied webhooks up front.
    let denied = require(&st, DEFAULT_TENANT, Feature::Webhooks)
        .await
        .unwrap_err();
    assert_eq!(denied.limit, "webhooks");

    let res = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("PUT")
                .uri(format!("/admin/tenants/{}/plan", DEFAULT_TENANT.0))
                .header("x-admin-token", ADMIN_TOKEN)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    serde_json::json!({ "plan": "starter" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::NO_CONTENT);

    // No wait for the TTL: the handler must have invalidated the cache.
    assert!(require(&st, DEFAULT_TENANT, Feature::Webhooks)
        .await
        .is_ok());
}

/// A tenant API token (`Scope::Full` even) must not be able to write its own
/// plan: only the break-glass `QUARK_ADMIN_TOKEN` compared directly is
/// accepted, never a credential `admin_guard` would otherwise resolve.
#[tokio::test]
#[serial_test::file_serial]
async fn tenant_cannot_promote_its_own_plan_without_the_break_glass_token() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    let st = state_with_plan_on_default_tenant("free").await;
    let app = quark::api::router(st.clone());
    let res = app
        .oneshot(
            axum::http::Request::builder()
                .method("PUT")
                .uri(format!("/admin/tenants/{}/plan", DEFAULT_TENANT.0))
                .header("x-admin-token", "not-the-real-token")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    serde_json::json!({ "plan": "custom" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::UNAUTHORIZED);
}

/// A typo in the plan string must fail loudly with `400`, not silently fall
/// back to Free the way `Plan::from_stored` does on read.
#[tokio::test]
#[serial_test::file_serial]
async fn unknown_plan_string_is_rejected_with_400() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    let st = state_with_plan_on_default_tenant("starter").await;
    let app = quark::api::router(st.clone());
    let res = app
        .oneshot(
            axum::http::Request::builder()
                .method("PUT")
                .uri(format!("/admin/tenants/{}/plan", DEFAULT_TENANT.0))
                .header("x-admin-token", ADMIN_TOKEN)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    serde_json::json!({ "plan": "starterr" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::BAD_REQUEST);
    // Still starter, unchanged.
    assert_eq!(
        quark::ee::plan::Plan::Starter,
        plan_of(&st, DEFAULT_TENANT).await
    );
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
