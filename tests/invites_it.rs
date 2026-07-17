//! P2c Task 1+2: `invites` table + store methods (Task 1), plus the
//! create/list/revoke HTTP endpoints (Task 2). Mirrors the non-superuser,
//! PG-gated harness in `tests/domains_it.rs`.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use quark::analytics::AnalyticsSink;
use quark::api::{router, AppState};
use quark::auth::{generate_token, hash_token, ApiToken, Scope, Session};
use quark::cache::Cache;
use quark::dns::NullDns;
use quark::invite::Invite;
use quark::store::postgres::PostgresStore;
use quark::store::{open_backends, Store};
use quark::tenant::{Membership, Role, Tenant, TenantId, User};
use quark::webhooks::delivery::WebhookDispatcher;
use serial_test::serial;
use std::sync::Arc;
use tower::ServiceExt;

async fn fresh() -> Option<PostgresStore> {
    let url = std::env::var("QUARK_TEST_DATABASE_URL").ok()?;
    let s = PostgresStore::open(&url, true).await.unwrap();
    s.reset_for_tests().await.unwrap();
    Some(s)
}

async fn make_tenant(store: &PostgresStore, slug: &str) -> TenantId {
    let id = store.next_tenant_id().await.unwrap();
    let tenant_id = TenantId(id);
    store
        .put_tenant(&Tenant {
            id: tenant_id,
            name: slug.to_string(),
            slug: slug.to_string(),
            created: 0,
        })
        .await
        .unwrap();
    tenant_id
}

async fn make_invite(
    store: &PostgresStore,
    tenant: TenantId,
    email: &str,
    raw_token: &str,
    created: u64,
    expires: u64,
) -> u64 {
    let id = store.next_invite_id().await.unwrap();
    store
        .create_invite(&Invite {
            id,
            tenant_id: tenant,
            email: email.to_string(),
            role: Role::Member,
            token_hash: hash_token(raw_token),
            invited_by: 1,
            created,
            expires,
            accepted_at: None,
            accepted_by: None,
        })
        .await
        .unwrap();
    id
}

/// The full lifecycle: create -> hash lookup finds it -> accept -> hash
/// lookup no longer finds it (accepted invites are invisible).
#[tokio::test]
#[serial]
async fn invite_accept_hides_it_from_hash_lookup() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let tenant = make_tenant(&store, "invites-accept-a").await;
    let id = make_invite(&store, tenant, "new@acme.com", "raw-token-1", 100, 1_000).await;

    let found = store
        .get_invite_by_hash(&hash_token("raw-token-1"), 200)
        .await
        .unwrap()
        .expect("pending invite must be found by its token hash");
    assert_eq!(found.id, id);
    assert_eq!(found.tenant_id, tenant);
    assert_eq!(found.email, "new@acme.com");
    assert_eq!(found.role, Role::Member);
    assert_eq!(found.accepted_at, None);
    assert_eq!(found.accepted_by, None);

    store.mark_invite_accepted(id, 42, 300).await.unwrap();

    assert!(
        store
            .get_invite_by_hash(&hash_token("raw-token-1"), 400)
            .await
            .unwrap()
            .is_none(),
        "an accepted invite must not be returned by hash lookup"
    );
}

/// `mark_invite_accepted` is a single-winner claim: once an invite is
/// accepted, a second call against the same row returns `false` rather than
/// silently re-accepting it. This is the store-level half of the TOCTOU fix;
/// the HTTP-level half is `accept_invite_grants_membership_and_repoints_session`
/// below, which asserts the second accept returns 404 and grants no second
/// membership.
#[tokio::test]
#[serial]
async fn mark_invite_accepted_is_single_winner() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let tenant = make_tenant(&store, "invites-claim-a").await;
    let id = make_invite(
        &store,
        tenant,
        "claim@acme.com",
        "raw-claim-token",
        quark::now(),
        quark::now() + 3600,
    )
    .await;

    let first = store
        .mark_invite_accepted(id, 42, quark::now())
        .await
        .unwrap();
    assert!(first, "the first claim on a pending invite must win");

    let second = store
        .mark_invite_accepted(id, 99, quark::now())
        .await
        .unwrap();
    assert!(
        !second,
        "a second claim on an already-accepted invite must lose"
    );
}

/// An invite whose `expires` is before `now` is invisible to the hash lookup,
/// even though it was never accepted.
#[tokio::test]
#[serial]
async fn expired_invite_is_invisible_to_hash_lookup() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let tenant = make_tenant(&store, "invites-expired-a").await;
    make_invite(&store, tenant, "late@acme.com", "raw-token-2", 100, 500).await;

    assert!(
        store
            .get_invite_by_hash(&hash_token("raw-token-2"), 501)
            .await
            .unwrap()
            .is_none(),
        "an expired invite must not be returned by hash lookup"
    );

    // Still not expired one tick earlier: `expires >= now` is inclusive.
    assert!(store
        .get_invite_by_hash(&hash_token("raw-token-2"), 500)
        .await
        .unwrap()
        .is_some());
}

/// `list_invites` is tenant-scoped: tenant B never sees tenant A's pending
/// invite.
#[tokio::test]
#[serial]
async fn list_invites_is_tenant_scoped() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let tenant_a = make_tenant(&store, "invites-list-a").await;
    let tenant_b = make_tenant(&store, "invites-list-b").await;
    make_invite(&store, tenant_a, "a@acme.com", "raw-token-3", 100, 1_000).await;

    let list_a = store.list_invites(tenant_a).await.unwrap();
    assert_eq!(list_a.len(), 1);
    assert_eq!(list_a[0].email, "a@acme.com");

    let list_b = store.list_invites(tenant_b).await.unwrap();
    assert!(
        list_b.is_empty(),
        "tenant B must not see tenant A's pending invite"
    );
}

/// `delete_invite` is tenant-scoped: tenant B cannot delete tenant A's
/// invite by id, and tenant A's own delete removes it from its list.
#[tokio::test]
#[serial]
async fn delete_invite_is_tenant_scoped() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let tenant_a = make_tenant(&store, "invites-delete-a").await;
    let tenant_b = make_tenant(&store, "invites-delete-b").await;
    let id = make_invite(&store, tenant_a, "del@acme.com", "raw-token-4", 100, 1_000).await;

    store.delete_invite(tenant_b, id).await.unwrap();
    assert_eq!(
        store.list_invites(tenant_a).await.unwrap().len(),
        1,
        "tenant B's delete attempt must not remove tenant A's invite"
    );

    store.delete_invite(tenant_a, id).await.unwrap();
    assert!(store.list_invites(tenant_a).await.unwrap().is_empty());
}

// --- Task 2: /admin/invites HTTP endpoints ---

const KEY: u64 = 0x1234;

fn test_webhook_dispatcher() -> Arc<WebhookDispatcher> {
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    Arc::new(WebhookDispatcher::new(
        tx,
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    ))
}

/// Builds a router for `tenant`, plus a `x-admin-token` API token with
/// `scopes` scoped to that tenant. `multi_tenant` toggles the cloud gate the
/// three invite endpoints share.
async fn admin_app_with_scopes(
    store: Arc<PostgresStore>,
    multi_tenant: bool,
    tenant: TenantId,
    token_id: u64,
    scopes: Vec<Scope>,
) -> (axum::Router, String) {
    let raw = format!("qtok_invites_test_{}", token_id);
    store
        .put_api_token(
            tenant,
            &ApiToken {
                id: token_id,
                name: "invites-test-token".to_string(),
                token_hash: hash_token(&raw),
                scopes,
                rate_limit_per_min: None,
                created: 0,
                tenant_id: tenant,
            },
        )
        .await
        .unwrap();

    let store_dyn: Arc<dyn Store> = store.clone();
    let sink_dyn: Arc<dyn AnalyticsSink> = store;
    let cache = Cache::new(store_dyn.clone(), 1000, None);
    let host_router = Arc::new(quark::domain_router::HostRouter::new(
        store_dyn.clone(),
        Some("quark.example.com".to_string()),
        None,
    ));
    let (analytics_tx, _rx) = tokio::sync::mpsc::channel(100);
    let state = Arc::new(AppState {
        oidc: None,
        sheets: None,
        sheets_api: None,
        oidc_configured: false,
        multi_tenant,
        tenant_domain_suffix: None,
        cache,
        store: store_dyn,
        key: KEY,
        signing_key: [0u8; 32],
        analytics_tx,
        sink: sink_dyn,
        admin_token: None,
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        block_private: true,
        public_host: Some("quark.example.com".to_string()),
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: test_webhook_dispatcher(),
        host_router,
        dns: Arc::new(NullDns),
    });
    (router(state), raw)
}

async fn create_invite(
    app: &axum::Router,
    token: &str,
    email: &str,
    role: &str,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::post("/admin/invites")
                .header("content-type", "application/json")
                .header("x-admin-token", token)
                .body(Body::from(format!(
                    r#"{{"email":"{email}","role":"{role}"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

/// Create with `role: "member"` returns a token in the body, and the store
/// only ever holds its hash (`token_hash != token`).
#[tokio::test]
#[serial]
async fn create_invite_returns_token_and_stores_only_hash() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "invites-http-create-a").await;
    let (app, token) =
        admin_app_with_scopes(store.clone(), true, tenant, 9101, vec![Scope::Full]).await;

    let (status, body) = create_invite(&app, &token, "new@acme.com", "member").await;
    assert_eq!(status, StatusCode::OK);
    let returned_token = body["token"].as_str().unwrap();
    assert!(!returned_token.is_empty());
    assert_eq!(body["email"], "new@acme.com");
    assert_eq!(body["role"], "member");
    assert!(body["expires"].as_u64().is_some());

    let stored = store.list_invites(tenant).await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_ne!(
        stored[0].token_hash, returned_token,
        "the store must hold only the token's hash, never the plaintext"
    );
    assert_eq!(stored[0].token_hash, hash_token(returned_token));
}

/// Create with `role: "owner"` is rejected: an invite can never grant
/// ownership.
#[tokio::test]
#[serial]
async fn create_invite_with_owner_role_is_400() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "invites-http-owner-a").await;
    let (app, token) =
        admin_app_with_scopes(store.clone(), true, tenant, 9102, vec![Scope::Full]).await;

    let (status, _) = create_invite(&app, &token, "boss@acme.com", "owner").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(store.list_invites(tenant).await.unwrap().is_empty());
}

/// Create with an empty (or whitespace-only) email is rejected: such an
/// invite could never be redeemed since accept matches on the session's
/// user email.
#[tokio::test]
#[serial]
async fn create_invite_with_empty_email_is_400() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "invites-http-empty-email-a").await;
    let (app, token) =
        admin_app_with_scopes(store.clone(), true, tenant, 9109, vec![Scope::Full]).await;

    let (status, _) = create_invite(&app, &token, "", "member").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status2, _) = create_invite(&app, &token, "   ", "member").await;
    assert_eq!(status2, StatusCode::BAD_REQUEST);

    assert!(store.list_invites(tenant).await.unwrap().is_empty());
}

/// A caller with insufficient scope (Viewer: links_read + analytics, no
/// `Scope::Full`) cannot create an invite -> 403.
#[tokio::test]
#[serial]
async fn create_invite_by_viewer_is_403() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "invites-http-viewer-a").await;
    let (app, token) = admin_app_with_scopes(
        store.clone(),
        true,
        tenant,
        9103,
        vec![Scope::LinksRead, Scope::Analytics],
    )
    .await;

    let (status, _) = create_invite(&app, &token, "someone@acme.com", "member").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(store.list_invites(tenant).await.unwrap().is_empty());
}

/// `GET /admin/invites` only returns the caller's tenant's pending invites,
/// and never the `token_hash` field.
#[tokio::test]
#[serial]
async fn list_invites_http_is_tenant_scoped_and_hides_hash() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant_a = make_tenant(&store, "invites-http-list-a").await;
    let tenant_b = make_tenant(&store, "invites-http-list-b").await;
    make_invite(
        &store,
        tenant_a,
        "a@acme.com",
        "raw-http-list-a",
        100,
        1_000,
    )
    .await;
    make_invite(
        &store,
        tenant_b,
        "b@acme.com",
        "raw-http-list-b",
        100,
        1_000,
    )
    .await;

    let (app_a, token_a) =
        admin_app_with_scopes(store.clone(), true, tenant_a, 9104, vec![Scope::Full]).await;

    let resp = app_a
        .clone()
        .oneshot(
            Request::get("/admin/invites")
                .header("x-admin-token", &token_a)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let list = json.as_array().unwrap();
    assert_eq!(list.len(), 1, "must only see tenant A's own pending invite");
    assert_eq!(list[0]["email"], "a@acme.com");
    assert!(
        list[0].get("token_hash").is_none(),
        "the list response must never include token_hash"
    );
}

/// `DELETE /admin/invites/:id` removes the invite.
#[tokio::test]
#[serial]
async fn delete_invite_http_removes_it() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "invites-http-delete-a").await;
    let id = make_invite(
        &store,
        tenant,
        "gone@acme.com",
        "raw-http-delete",
        100,
        1_000,
    )
    .await;
    let (app, token) =
        admin_app_with_scopes(store.clone(), true, tenant, 9105, vec![Scope::Full]).await;

    let resp = app
        .clone()
        .oneshot(
            Request::delete(format!("/admin/invites/{id}"))
                .header("x-admin-token", &token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(store.list_invites(tenant).await.unwrap().is_empty());

    // Deleting again (already gone) -> 404.
    let resp = app
        .clone()
        .oneshot(
            Request::delete(format!("/admin/invites/{id}"))
                .header("x-admin-token", &token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// All three `/admin/invites` endpoints 404 when `multi_tenant = false`.
#[tokio::test]
#[serial]
async fn invites_endpoints_404_in_oss() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "invites-http-oss-a").await;
    let (app, token) =
        admin_app_with_scopes(store.clone(), false, tenant, 9106, vec![Scope::Full]).await;

    let (status, _) = create_invite(&app, &token, "x@acme.com", "member").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let resp = app
        .clone()
        .oneshot(
            Request::get("/admin/invites")
                .header("x-admin-token", &token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let resp = app
        .clone()
        .oneshot(
            Request::delete("/admin/invites/1")
                .header("x-admin-token", &token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// --- Task 3: POST /admin/invites/:token/accept ---

/// Builds a router over `store` with a session-based (OIDC) principal, the
/// shape the accept endpoint reads via `session_user_id` rather than
/// `admin_guard`'s `x-admin-token` path used by create/list/revoke above.
fn session_app_over(store: Arc<PostgresStore>, multi_tenant: bool) -> axum::Router {
    let store_dyn: Arc<dyn Store> = store.clone();
    let sink_dyn: Arc<dyn AnalyticsSink> = store;
    let cache = Cache::new(store_dyn.clone(), 1000, None);
    let host_router = Arc::new(quark::domain_router::HostRouter::new(
        store_dyn.clone(),
        None,
        None,
    ));
    let (analytics_tx, _rx) = tokio::sync::mpsc::channel(100);
    let state = Arc::new(AppState {
        oidc: None,
        sheets: None,
        sheets_api: None,
        oidc_configured: true,
        multi_tenant,
        tenant_domain_suffix: None,
        cache,
        store: store_dyn,
        key: KEY,
        signing_key: [0u8; 32],
        analytics_tx,
        sink: sink_dyn,
        admin_token: None,
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: test_webhook_dispatcher(),
        host_router,
        dns: Arc::new(NullDns),
    });
    router(state)
}

/// Seeds a user + a session for it, returning `(user_id, raw_cookie_value)`.
async fn seed_session(store: &PostgresStore, subject: &str, email: &str) -> (u64, String) {
    let user_id = store.next_user_id().await.unwrap();
    store
        .put_user(&User {
            id: user_id,
            subject: subject.to_string(),
            email: email.to_string(),
            display: subject.to_string(),
            created: 0,
        })
        .await
        .unwrap();
    let raw = generate_token();
    let session = Session {
        token_hash: hash_token(&raw),
        subject: subject.to_string(),
        display: subject.to_string(),
        scopes: vec![],
        created: 0,
        expires: quark::now() + 3600,
        tenant_id: TenantId(0),
        user_id,
    };
    store.put_session(TenantId(0), &session).await.unwrap();
    (user_id, raw)
}

/// Runs `POST /admin/invites/:token/accept`, optionally with a session
/// cookie, returning `(status, body_json)`.
async fn accept_invite(
    app: &axum::Router,
    token: &str,
    session_cookie: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let mut req = Request::post(format!("/admin/invites/{token}/accept"));
    if let Some(raw) = session_cookie {
        req = req.header("cookie", format!("qk_session={raw}"));
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

/// Happy path: the session's user email matches the invite's target email,
/// so accepting grants the invited role, marks the invite accepted (a
/// second accept then 404s), and re-points the session's tenant.
#[tokio::test]
#[serial]
async fn accept_invite_grants_membership_and_repoints_session() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "invites-accept-http-a").await;
    let (_user_id, raw) = seed_session(&store, "accept-happy", "new@acme.com").await;
    make_invite(
        &store,
        tenant,
        "new@acme.com",
        "raw-accept-happy",
        quark::now(),
        quark::now() + 3600,
    )
    .await;

    let app = session_app_over(store.clone(), true);
    let (status, body) = accept_invite(&app, "raw-accept-happy", Some(&raw)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["tenant_id"], tenant.0);
    assert_eq!(body["role"], "member");

    let membership = store.get_membership(_user_id, tenant).await.unwrap();
    assert_eq!(
        membership.map(|m| m.role),
        Some(Role::Member),
        "accepting must grant the invited role"
    );

    // Single-use: accepting the same token again 404s (already accepted).
    let (status2, _) = accept_invite(&app, "raw-accept-happy", Some(&raw)).await;
    assert_eq!(status2, StatusCode::NOT_FOUND);
}

/// A wrong/unknown token 404s.
#[tokio::test]
#[serial]
async fn accept_invite_wrong_token_is_404() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let (_user_id, raw) = seed_session(&store, "accept-wrong-token", "someone@acme.com").await;

    let app = session_app_over(store.clone(), true);
    let (status, _) = accept_invite(&app, "never-issued-token", Some(&raw)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// An expired invite 404s (never accepted, just past `expires`).
#[tokio::test]
#[serial]
async fn accept_invite_expired_is_404() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "invites-accept-http-expired").await;
    let (_user_id, raw) = seed_session(&store, "accept-expired", "late@acme.com").await;
    // expires in the past relative to `quark::now()`.
    make_invite(&store, tenant, "late@acme.com", "raw-accept-expired", 1, 2).await;

    let app = session_app_over(store.clone(), true);
    let (status, _) = accept_invite(&app, "raw-accept-expired", Some(&raw)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// A session whose `User.email` does not match the invite's target email is
/// rejected with 403, and no membership is granted.
#[tokio::test]
#[serial]
async fn accept_invite_email_mismatch_is_403() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "invites-accept-http-mismatch").await;
    let (user_id, raw) = seed_session(&store, "accept-mismatch", "someone-else@acme.com").await;
    make_invite(
        &store,
        tenant,
        "intended@acme.com",
        "raw-accept-mismatch",
        quark::now(),
        quark::now() + 3600,
    )
    .await;

    let app = session_app_over(store.clone(), true);
    let (status, _) = accept_invite(&app, "raw-accept-mismatch", Some(&raw)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(store
        .get_membership(user_id, tenant)
        .await
        .unwrap()
        .is_none());
}

/// A user who already has a membership on the invite's tenant gets 409, and
/// the existing membership's role is left untouched.
#[tokio::test]
#[serial]
async fn accept_invite_already_member_is_409() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "invites-accept-http-already").await;
    let (user_id, raw) = seed_session(&store, "accept-already", "member@acme.com").await;
    store
        .put_membership(&Membership {
            user_id,
            tenant_id: tenant,
            role: Role::Viewer,
            created: 0,
        })
        .await
        .unwrap();
    make_invite(
        &store,
        tenant,
        "member@acme.com",
        "raw-accept-already",
        quark::now(),
        quark::now() + 3600,
    )
    .await;

    let app = session_app_over(store.clone(), true);
    let (status, _) = accept_invite(&app, "raw-accept-already", Some(&raw)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let membership = store.get_membership(user_id, tenant).await.unwrap();
    assert_eq!(
        membership.map(|m| m.role),
        Some(Role::Viewer),
        "an already-member's existing role must not be overwritten"
    );
}

/// No session cookie at all -> 401, before the invite is even looked up.
#[tokio::test]
#[serial]
async fn accept_invite_no_session_is_401() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "invites-accept-http-nosession").await;
    make_invite(
        &store,
        tenant,
        "whoever@acme.com",
        "raw-accept-nosession",
        100,
        1_000_000,
    )
    .await;

    let app = session_app_over(store.clone(), true);
    let (status, _) = accept_invite(&app, "raw-accept-nosession", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// `multi_tenant = false` (OSS) -> 404, same gate as create/list/revoke.
#[tokio::test]
#[serial]
async fn accept_invite_404_in_oss() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let (_user_id, raw) = seed_session(&store, "accept-oss", "whoever@acme.com").await;

    let app = session_app_over(store.clone(), false);
    let (status, _) = accept_invite(&app, "raw-whatever", Some(&raw)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// --- Task 4: OSS parity (ungated, LMDB) + security sweep -------------------
//
// The test below runs over an LMDB-backed store, never gated on
// `QUARK_TEST_DATABASE_URL`: the point is that OSS deployments (no Postgres,
// no cloud flag) get a 404 on every invite surface without needing a test
// database at all, mirroring `oss_workspace_endpoints_are_404` in
// `tests/workspace_it.rs`.

/// Builds a full quark router over a dyn `Store`, with a given `multi_tenant`
/// mode. Used only for the ungated LMDB test below.
fn app_over(
    store: Arc<dyn Store>,
    sink: Arc<dyn quark::analytics::AnalyticsSink>,
    multi_tenant: bool,
) -> axum::Router {
    let cache = Cache::new(store.clone(), 1000, None);
    let host_router = Arc::new(quark::domain_router::HostRouter::new(
        store.clone(),
        None,
        None,
    ));
    let (analytics_tx, _rx) = tokio::sync::mpsc::channel(100);
    let state = Arc::new(AppState {
        oidc: None,
        sheets: None,
        sheets_api: None,
        oidc_configured: true,
        multi_tenant,
        tenant_domain_suffix: None,
        cache,
        store,
        key: KEY,
        signing_key: [0u8; 32],
        analytics_tx,
        sink,
        admin_token: None,
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: test_webhook_dispatcher(),
        host_router,
        dns: Arc::new(NullDns),
    });
    router(state)
}

/// All three `/admin/invites` surfaces (`POST /admin/invites`,
/// `GET /admin/invites`, `POST /admin/invites/:token/accept`) 404 in OSS
/// (`multi_tenant = false`) with no Postgres configured at all and no
/// credential presented: the flag gate runs before authentication, so this
/// must always run, not just when a test Postgres happens to be available.
#[tokio::test]
async fn oss_invites_endpoints_are_404_without_postgres() {
    let dir = tempfile::tempdir().unwrap();
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    let app = app_over(store, sink, false);

    let resp = app
        .clone()
        .oneshot(
            Request::post("/admin/invites")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"email":"x@acme.com","role":"member"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let resp = app
        .clone()
        .oneshot(Request::get("/admin/invites").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let resp = app
        .oneshot(
            Request::post("/admin/invites/whatever-token/accept")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// `GET /admin/invites` and `DELETE /admin/invites/:id`, exercised at the HTTP
/// layer with a real (non-superuser) per-tenant API token: tenant B's token
/// never sees tenant A's pending invite in the list, and a delete attempt
/// against tenant A's invite id 404s exactly as an unknown id would (the
/// store-level equivalent is already covered by `list_invites_is_tenant_scoped`
/// and `delete_invite_is_tenant_scoped` above; this closes the gap at the HTTP
/// boundary, where `admin_guard` resolves the tenant from the caller's own
/// token rather than a test taking it on faith).
#[tokio::test]
#[serial]
async fn list_and_delete_invites_http_are_tenant_scoped_for_non_superuser() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant_a = make_tenant(&store, "invites-http-scope-a").await;
    let tenant_b = make_tenant(&store, "invites-http-scope-b").await;
    let invite_id = make_invite(
        &store,
        tenant_a,
        "scoped@acme.com",
        "raw-http-scope-a",
        100,
        1_000,
    )
    .await;

    let (_app_a, _token_a) =
        admin_app_with_scopes(store.clone(), true, tenant_a, 9107, vec![Scope::Full]).await;
    let (app_b, token_b) =
        admin_app_with_scopes(store.clone(), true, tenant_b, 9108, vec![Scope::Full]).await;

    let resp = app_b
        .clone()
        .oneshot(
            Request::get("/admin/invites")
                .header("x-admin-token", &token_b)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        json.as_array().unwrap().is_empty(),
        "tenant B must not see tenant A's pending invite over HTTP"
    );

    let resp = app_b
        .oneshot(
            Request::delete(format!("/admin/invites/{invite_id}"))
                .header("x-admin-token", &token_b)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "tenant B must not be able to delete tenant A's invite over HTTP"
    );

    // Tenant A's own invite is untouched by tenant B's failed attempt.
    assert_eq!(store.list_invites(tenant_a).await.unwrap().len(), 1);
}
