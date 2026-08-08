// Codigo de teste pode entrar em panico: a falha e o proprio sinal.
#![allow(clippy::unwrap_used)]
// Enterprise suite: plans only exist in the `--features ee` build (LUC-41).
#![cfg(feature = "ee")]

use quark::analytics::AnalyticsSink;
use quark::api::entitlement::{require, require_quota, Feature, Quota};
use quark::auth::{hash_token, ApiToken, Scope};
use quark::ee::api::entitlement::plan_of;
use quark::store::postgres::PostgresStore;
use quark::store::Store;
use quark::tenant::{Membership, Role, Tenant, TenantId, DEFAULT_TENANT};
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

/// End-to-end proof that `POST /admin/invites` is wired to `Quota::Members`,
/// mirroring `free_tenant_gets_402_on_the_fourth_domain`: Free allows 1
/// member. The tenant starts holding 0 (only `count_memberships` counts,
/// never a pending invite), so the first invite is accepted; seeding one real
/// membership row afterwards puts the tenant at the ceiling, and the next
/// invite is denied with `402`.
#[tokio::test]
#[serial_test::file_serial]
async fn free_tenant_gets_402_creating_a_second_invite_at_the_member_ceiling() {
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
    let st = common::TestState::new(store.clone(), sink)
        .admin_token(Some(ADMIN_TOKEN.into()))
        .multi_tenant(true)
        .build();
    let app = quark::api::router(st.clone());

    async fn create_invite(app: &axum::Router, email: &str) -> axum::http::StatusCode {
        app.clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/admin/invites")
                    .header("x-admin-token", ADMIN_TOKEN)
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::json!({ "email": email, "role": "member" }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }

    // Holding 0 members is under Free's ceiling of 1.
    assert_eq!(
        create_invite(&app, "first@acme.com").await,
        axum::http::StatusCode::OK
    );
    // Seed a real membership (e.g. the tenant's own Owner) to reach the
    // ceiling. A pending invite alone must never count; only this does.
    store
        .put_membership(&Membership {
            user_id: 999,
            tenant_id: DEFAULT_TENANT,
            role: Role::Owner,
            created: 0,
        })
        .await
        .unwrap();
    assert_eq!(
        create_invite(&app, "second@acme.com").await,
        axum::http::StatusCode::PAYMENT_REQUIRED
    );
}

/// End-to-end proof that `GET /admin/integrations/sheets/connect` is wired to
/// `Feature::Integrations`: a Free-plan tenant gets `402`, not the connect
/// URL, and the gate fires before the "connector not configured" check (so
/// this needs no `st.sheets` setup at all).
#[tokio::test]
#[serial_test::file_serial]
async fn free_tenant_gets_402_on_sheets_connect() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    let st = state_with_plan_on_default_tenant("free").await;
    let app = quark::api::router(st.clone());
    let res = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/admin/integrations/sheets/connect")
                .header("x-admin-token", ADMIN_TOKEN)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::PAYMENT_REQUIRED);
}

/// Same proof for `POST /admin/pixels`: `Feature::Integrations` is checked
/// before the request body is even parsed, so a Free-plan tenant gets `402`
/// regardless of what the body contains.
#[tokio::test]
#[serial_test::file_serial]
async fn free_tenant_gets_402_creating_a_pixel() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    let st = state_with_plan_on_default_tenant("free").await;
    let app = quark::api::router(st.clone());
    let body = serde_json::json!({
        "provider": "ga4",
        "credentials": { "measurement_id": "G-TEST", "api_secret": "s3cr3t" },
    });
    let res = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/admin/pixels")
                .header("x-admin-token", ADMIN_TOKEN)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::PAYMENT_REQUIRED);
}

/// End-to-end proof that `PUT /admin/oidc-config` is wired to `Feature::Sso`:
/// a Free-plan tenant gets `402`, not the config written, and nothing is
/// persisted.
#[tokio::test]
#[serial_test::file_serial]
async fn free_tenant_gets_402_on_oidc_config_put() {
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
    let st = common::TestState::new(store.clone(), sink)
        .admin_token(Some(ADMIN_TOKEN.into()))
        .multi_tenant(true)
        .build();
    let app = quark::api::router(st.clone());
    let body = serde_json::json!({
        "issuer": "https://idp.acme.example",
        "client_id": "acme-client",
        "client_secret": "top-secret-value",
    });
    let res = app
        .oneshot(
            axum::http::Request::builder()
                .method("PUT")
                .uri("/admin/oidc-config")
                .header("x-admin-token", ADMIN_TOKEN)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::PAYMENT_REQUIRED);
    assert!(
        store
            .get_oidc_config(DEFAULT_TENANT)
            .await
            .unwrap()
            .is_none(),
        "a denied PUT must not persist a config"
    );
}

/// LMDB (the embedded, single-binary backend) has no plan system at all:
/// `get_tenant_plan` can only ever answer `Ok(None)` and `set_tenant_plan` is
/// `Unsupported`. `plan_of` must read that as "this backend cannot carry a
/// plan" and answer `Plan::Custom` (unlimited), not silently fall through to
/// the same `Ok(None)` handling Postgres uses for "no plan row yet" (which
/// resolves to `Free`). That would deny an Enterprise self-hosted install
/// (embedded store, `--features ee`) every feature it already paid for.
///
/// Ungated: LMDB is the default backend when `QUARK_DATABASE_URL` is unset,
/// so this needs no `QUARK_TEST_DATABASE_URL`, mirroring
/// `oss_invites_endpoints_are_404_without_postgres` in `tests/invites_it.rs`.
#[tokio::test]
async fn lmdb_backend_with_no_plan_system_resolves_to_unlimited_custom() {
    let dir = tempfile::tempdir().unwrap();
    let (store, sink) = quark::store::open_backends(dir.path(), true).await.unwrap();
    let st = common::TestState::new(store, sink).build();
    let tenant = TenantId(4242);

    assert_eq!(plan_of(&st, tenant).await, quark::ee::plan::Plan::Custom);
    assert!(require(&st, tenant, Feature::Sso).await.is_ok());
    assert!(require(&st, tenant, Feature::Webhooks).await.is_ok());
    assert!(require_quota(&st, tenant, Quota::Members, 999_999)
        .await
        .is_ok());
}

/// The break-glass check in `admin_tenant_plan_put` is a manual
/// `constant_time_eq` against `st.admin_token`, deliberately NOT
/// `admin_guard`: a real per-tenant API token with `Scope::Full`, the highest
/// scope `admin_guard` would ever resolve, must still be rejected. This pins
/// the regression `tenant_cannot_promote_its_own_plan_without_the_break_glass_token`
/// cannot catch (an invented string is a weaker case than a real, valid
/// credential): swapping the manual comparison for `admin_guard(&st,
/// &headers, Scope::Full)` would silently start accepting this token and let
/// a tenant promote itself to `custom`, and this test would catch that where
/// the other one would stay green.
#[tokio::test]
#[serial_test::file_serial]
async fn tenant_api_token_with_full_scope_cannot_promote_its_own_plan() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    let st = state_with_plan_on_default_tenant("free").await;
    let raw = "qtok_plan_promote_test";
    st.store
        .put_api_token(
            DEFAULT_TENANT,
            &ApiToken {
                id: 42,
                name: "full-scope-tenant-token".to_string(),
                token_hash: hash_token(raw),
                scopes: vec![Scope::Full],
                rate_limit_per_min: None,
                created: 0,
                tenant_id: DEFAULT_TENANT,
            },
        )
        .await
        .unwrap();
    let app = quark::api::router(st.clone());

    let res = app
        .oneshot(
            axum::http::Request::builder()
                .method("PUT")
                .uri(format!("/admin/tenants/{}/plan", DEFAULT_TENANT.0))
                .header("x-admin-token", raw)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    serde_json::json!({ "plan": "custom" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::UNAUTHORIZED);
    assert_eq!(
        plan_of(&st, DEFAULT_TENANT).await,
        quark::ee::plan::Plan::Free,
        "a rejected promotion must leave the tenant's plan untouched"
    );
}
