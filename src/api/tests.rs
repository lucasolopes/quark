use super::{
    access_log_line, app_destination, cache_control_for, classify_platform, create_link_core,
    fbclid_from_query, normalize_max_visits, parse_cors_origins, resolve_code, resolve_for_admin,
    send_test_event_guarded, EventType, Platform, SubscriptionKind, WebhookSubscription,
    HEADER_ADMIN_TOKEN, SHARED_DOMAIN_ID,
};
use crate::store::Record;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap as ReqHeaderMap;
use axum::routing::any;
use axum::Router as TestRouter;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::net::TcpListener;

fn rec(app_ios: Option<&str>, app_android: Option<&str>) -> Record {
    Record {
        url: "https://example.com".into(),
        expiry: None,
        created: 0,
        tags: Vec::new(),
        max_visits: None,
        rules: Vec::new(),
        variants: Vec::new(),
        app_ios: app_ios.map(str::to_string),
        app_android: app_android.map(str::to_string),
        folder: None,
        fallback_url: None,
        password_hash: None,
        tenant_id: crate::tenant::DEFAULT_TENANT,
    }
}

/// Minimal `AppState` for exercising `admin_guard` directly: LMDB-backed
/// store (so API tokens can be inserted), no OIDC/sheets, rate limiter
/// disabled. `admin_token` sets (or clears) the env break-glass token.
async fn guard_state(admin_token: Option<&str>) -> Arc<super::AppState> {
    guard_state_with_oidc(admin_token, false).await
}

/// Same as `guard_state`, but lets the caller control `oidc_configured`
/// (needed to exercise the OIDC-gated session paths, e.g.
/// `session_user_id`, without wiring a real IdP).
async fn guard_state_with_oidc(
    admin_token: Option<&str>,
    oidc_configured: bool,
) -> Arc<super::AppState> {
    build_state(admin_token, oidc_configured, false, [0u8; 32]).await
}

/// Cloud-mode `AppState` for exercising the `?org=` login/callback
/// decision logic (multi-tenancy P2d) without a live IdP: LMDB-backed
/// (`multi_tenant: true`), no global env OIDC configured. LMDB's
/// `get_oidc_config_bare`/`get_oidc_config` always return `Ok(None)` (see
/// `src/store/lmdb.rs`), which is exactly the "tenant exists but has no
/// IdP of its own" shape these tests need — the store-lookup and
/// tenant-resolution branches never require reaching a real IdP over the
/// network.
async fn multi_tenant_state() -> Arc<super::AppState> {
    build_state(None, false, true, [7u8; 32]).await
}

/// Shared LMDB-backed `AppState` builder for the unit tests above. The two
/// public-to-the-module helpers differ only in these four axes; everything
/// else (no OIDC/sheets/keycloak, disabled rate limiter, no-op webhook
/// dispatcher with a dropped receiver) is fixed.
async fn build_state(
    admin_token: Option<&str>,
    oidc_configured: bool,
    multi_tenant: bool,
    signing_key: [u8; 32],
) -> Arc<super::AppState> {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = crate::store::open_backends(dir.path(), false)
        .await
        .unwrap();
    let cache = crate::cache::Cache::new(store.clone(), 1000, None);
    let host_router = Arc::new(crate::domain_router::HostRouter::new(
        store.clone(),
        None,
        None,
    ));
    let (analytics_tx, _rx) = tokio::sync::mpsc::channel(100);
    let (tx, _wrx) = tokio::sync::mpsc::channel(1);
    let webhooks = Arc::new(crate::webhooks::delivery::WebhookDispatcher::new(
        tx,
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    ));
    Arc::new(super::AppState {
        oidc: None,
        sheets: None,
        sheets_api: None,
        oidc_configured,
        cache,
        store,
        key: 0x1234,
        signing_key,
        analytics_tx,
        sink,
        admin_token: admin_token.map(str::to_string),
        ratelimiter: crate::abuse::ratelimit::RateLimiter::disabled(),
        block_private: true,
        public_host: None,
        real_ip_header: super::DEFAULT_REAL_IP_HEADER.to_string(),
        webhooks,
        multi_tenant,
        host_router,
        dns: Arc::new(crate::dns::NullDns),
        tenant_domain_suffix: None,
        oidc_tenants: crate::oidc::TenantOidcCache::new(),
        keycloak: None,
        keycloak_base_url: None,
    })
}

// --- `?org=` login / per-tenant callback (multi-tenancy P2d) ---
//
// These exercise the resolution/decision logic directly against the
// handlers: which tenant (if any) a login resolves to, and whether the
// outcome is the explicit error the security model requires (never a
// silent fallthrough to a different IdP). None of the cases below need a
// live IdP: `?org=` on an unknown slug or a tenant with no config of its
// own is rejected before any network call would happen. The "known slug
// WITH a working config" happy path additionally needs the tenant's IdP
// to actually answer discovery/JWKS/token requests, which the LMDB test
// backend has no way to provide (`get_oidc_config_bare` always returns
// `Ok(None)` there); that path is covered by the Postgres-gated store
// tests (`tests/oidc_config_it.rs`) for config storage/isolation, plus
// the `oidc.rs` unit tests for cookie signing/claim mapping/membership.
// Exercising the full network round trip needs a real or fake IdP
// (Keycloak, per `docker-compose.e2e.yml`) and is deferred to the P2d
// frontend/e2e follow-up; see the Task 4 report.

#[tokio::test]
async fn org_login_unknown_slug_is_404_not_global() {
    let st = multi_tenant_state().await;
    let resp = super::oidc_login(
        State(st),
        axum::extract::Query(super::LoginParams {
            org: Some("ghost-org".to_string()),
            login_hint: None,
        }),
        ReqHeaderMap::new(),
    )
    .await;
    assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
    assert!(
        resp.headers().get(axum::http::header::LOCATION).is_none(),
        "an unknown org must never redirect to any IdP"
    );
}

#[tokio::test]
async fn org_login_tenant_without_oidc_config_is_404_not_global() {
    let st = multi_tenant_state().await;
    let tenant_id = crate::tenant::TenantId(st.store.next_tenant_id().await.unwrap());
    st.store
        .put_tenant(&crate::tenant::Tenant {
            id: tenant_id,
            name: "Acme".to_string(),
            slug: "acme".to_string(),
            created: 0,
        })
        .await
        .unwrap();

    let resp = super::oidc_login(
        State(st),
        axum::extract::Query(super::LoginParams {
            org: Some("acme".to_string()),
            login_hint: None,
        }),
        ReqHeaderMap::new(),
    )
    .await;
    assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
    assert!(
        resp.headers().get(axum::http::header::LOCATION).is_none(),
        "a tenant with no OIDC config of its own must never fall back to the global IdP"
    );
}

/// Slug-enumeration close (multi-tenancy P2d Task 4b): an unknown slug
/// and a real tenant with no OIDC config of its own must return the
/// exact same 404 body. Before this fix they carried distinct messages
/// ("unknown organization" vs "organization has no identity provider
/// configured"), letting an unauthenticated caller tell real slugs apart
/// from made-up ones one probe at a time.
#[tokio::test]
async fn org_login_unknown_slug_and_unconfigured_tenant_return_identical_404_body() {
    let st = multi_tenant_state().await;
    let tenant_id = crate::tenant::TenantId(st.store.next_tenant_id().await.unwrap());
    st.store
        .put_tenant(&crate::tenant::Tenant {
            id: tenant_id,
            name: "Acme".to_string(),
            slug: "acme".to_string(),
            created: 0,
        })
        .await
        .unwrap();

    let unknown_resp = super::oidc_login(
        State(st.clone()),
        axum::extract::Query(super::LoginParams {
            org: Some("ghost-org".to_string()),
            login_hint: None,
        }),
        ReqHeaderMap::new(),
    )
    .await;
    let unconfigured_resp = super::oidc_login(
        State(st),
        axum::extract::Query(super::LoginParams {
            org: Some("acme".to_string()),
            login_hint: None,
        }),
        ReqHeaderMap::new(),
    )
    .await;

    assert_eq!(unknown_resp.status(), axum::http::StatusCode::NOT_FOUND);
    assert_eq!(
        unconfigured_resp.status(),
        axum::http::StatusCode::NOT_FOUND
    );
    let unknown_body = axum::body::to_bytes(unknown_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let unconfigured_body = axum::body::to_bytes(unconfigured_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        unknown_body, unconfigured_body,
        "unknown slug and unconfigured tenant must be indistinguishable"
    );
}

#[tokio::test]
async fn org_login_requires_multi_tenant_mode() {
    let st = guard_state_with_oidc(None, false).await; // multi_tenant: false
    let resp = super::oidc_login(
        State(st),
        axum::extract::Query(super::LoginParams {
            org: Some("acme".to_string()),
            login_hint: None,
        }),
        ReqHeaderMap::new(),
    )
    .await;
    assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn org_login_absent_is_the_unchanged_global_path() {
    // multi_tenant: true, but no `?org=` and no global OIDC configured:
    // behaves exactly like the pre-P2d global path (404, oidc not
    // configured), regardless of the cloud/OSS deployment mode.
    let st = multi_tenant_state().await;
    let resp = super::oidc_login(
        State(st),
        axum::extract::Query(super::LoginParams {
            org: None,
            login_hint: None,
        }),
        ReqHeaderMap::new(),
    )
    .await;
    assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn org_login_empty_string_is_treated_as_absent() {
    let st = multi_tenant_state().await;
    let resp = super::oidc_login(
        State(st),
        axum::extract::Query(super::LoginParams {
            org: Some(String::new()),
            login_hint: None,
        }),
        ReqHeaderMap::new(),
    )
    .await;
    // Same outcome as `org: None`: falls to the global path (404 here,
    // since no global OIDC is configured), not treated as a slug lookup.
    assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
}

// Status-contract restore (multi-tenancy P2d Task 4b): before per-tenant
// login existed, `oidc_callback` checked `st.oidc.is_none()` first,
// unconditionally, so a request against a deployment with no global
// OIDC configured was always 404 ("oidc not configured") — regardless
// of `?error=`, cookie presence, `state`, or `code`. The P2d refactor
// moved that check inside the `None`-tenant match arm, so it only fired
// after the error/cookie/state/code checks had already returned their
// own (401/400) status first. These two tests pin the contract back to
// 404 for the two ways a request resolves to "no tenant, no global IdP".
#[tokio::test]
async fn callback_no_global_oidc_missing_cookie_is_404() {
    let st = guard_state_with_oidc(None, false).await;
    let resp = super::oidc_callback(
        State(st),
        axum::extract::Query(super::CallbackParams {
            code: Some("some-code".to_string()),
            state: Some("some-state".to_string()),
            error: None,
        }),
        ReqHeaderMap::new(),
    )
    .await;
    assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn callback_no_global_oidc_with_error_param_is_404_not_401() {
    // Even an IdP-supplied `?error=` must not preempt the "no OIDC
    // configured at all" 404 when there is no tenant to fall back on.
    let st = guard_state_with_oidc(None, false).await;
    let resp = super::oidc_callback(
        State(st),
        axum::extract::Query(super::CallbackParams {
            code: None,
            state: None,
            error: Some("access_denied".to_string()),
        }),
        ReqHeaderMap::new(),
    )
    .await;
    assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn callback_tenant_from_cookie_but_config_gone_is_400_not_global() {
    // The tenant signed into the cookie no longer has an OIDC config
    // (e.g. removed mid-flow, or a forged tenant id that happens to
    // exist but was never configured). This must be an explicit error,
    // never a fall-through to the global IdP.
    let st = multi_tenant_state().await;
    let tenant_id = crate::tenant::TenantId(st.store.next_tenant_id().await.unwrap());
    st.store
        .put_tenant(&crate::tenant::Tenant {
            id: tenant_id,
            name: "Acme".to_string(),
            slug: "acme".to_string(),
            created: 0,
        })
        .await
        .unwrap();
    let cookie_value =
        crate::oidc::sign_login_state(&st.signing_key, "st8", "verif", "nnc", Some(tenant_id));
    let mut headers = ReqHeaderMap::new();
    headers.insert(
        axum::http::header::COOKIE,
        format!("qk_login={cookie_value}").parse().unwrap(),
    );
    let resp = super::oidc_callback(
        State(st),
        axum::extract::Query(super::CallbackParams {
            code: Some("code".to_string()),
            state: Some("st8".to_string()),
            error: None,
        }),
        headers,
    )
    .await;
    assert_eq!(resp.status(), axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn callback_tampered_tenant_in_cookie_is_rejected() {
    // A cookie whose tenant field was swapped for a different tenant id
    // must fail the HMAC check entirely (verified at the `oidc.rs`
    // level), so no tenant can be trusted out of it — at the HTTP layer
    // that is indistinguishable from no cookie at all, which (per the
    // restored status-contract test above) is 404 here since this
    // deployment has no global OIDC configured either. Either way, the
    // swapped-in tenant is never authenticated into.
    let st = multi_tenant_state().await;
    let real_tenant = crate::tenant::TenantId(1);
    let cookie_value =
        crate::oidc::sign_login_state(&st.signing_key, "st8", "verif", "nnc", Some(real_tenant));
    let tampered = cookie_value.replacen(".1.", ".2.", 1);
    assert_ne!(
        tampered, cookie_value,
        "sanity: tamper must actually change the value"
    );
    let mut headers = ReqHeaderMap::new();
    headers.insert(
        axum::http::header::COOKIE,
        format!("qk_login={tampered}").parse().unwrap(),
    );
    let resp = super::oidc_callback(
        State(st),
        axum::extract::Query(super::CallbackParams {
            code: Some("code".to_string()),
            state: Some("st8".to_string()),
            error: None,
        }),
        headers,
    )
    .await;
    assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
}

/// `claim_role` never grants `Role::Owner`, and its Admin/Viewer/Member
/// mapping matches the `TenantOidcConfig`'s claim, end to end through
/// `ensure_user_and_membership` with a real store (LMDB) — the same path
/// `oidc_callback` drives for a per-tenant login. This is the decision
/// logic the HTTP callback cannot exercise without a live IdP, tested
/// directly instead.
#[tokio::test]
async fn tenant_login_membership_role_matches_claim_mapping() {
    let st = multi_tenant_state().await;
    let cfg = crate::oidc::TenantOidcConfig {
        tenant_id: crate::tenant::TenantId(1),
        issuer: "https://idp.acme.example".into(),
        client_id: "acme".into(),
        client_secret: "s".into(),
        scopes: vec!["openid".into()],
        admin_claim: "groups".into(),
        admin_value: "acme-admins".into(),
        readonly_value: "acme-viewers".into(),
        member_value: String::new(),
        required_value: None,
        post_login_url: None,
        post_logout_url: None,
    };
    let tenant = crate::tenant::TenantId(1);

    let admin_claims = serde_json::json!({ "groups": ["acme-admins"] });
    let role = crate::oidc::claim_role(&admin_claims, &cfg);
    assert_eq!(role, crate::tenant::Role::Admin);
    let uid = crate::oidc::ensure_user_and_membership(
        st.store.as_ref(),
        true,
        "sub-a",
        "a@acme.example",
        "A",
        &[],
        Some((tenant, role)),
    )
    .await
    .unwrap();
    let m = st.store.get_membership(uid, tenant).await.unwrap().unwrap();
    assert_eq!(m.role, crate::tenant::Role::Admin);

    let viewer_claims = serde_json::json!({ "groups": ["acme-viewers"] });
    let role = crate::oidc::claim_role(&viewer_claims, &cfg);
    assert_eq!(role, crate::tenant::Role::Viewer);

    let neither_claims = serde_json::json!({ "groups": ["nobody"] });
    let role = crate::oidc::claim_role(&neither_claims, &cfg);
    assert_eq!(role, crate::tenant::Role::Member);
    assert_ne!(role, crate::tenant::Role::Owner);
}

/// A login into tenant A's own OIDC creates a membership ONLY in A, never
/// in any other tenant — same decision-logic level as
/// `tenant_login_membership_role_matches_claim_mapping`, but asserting
/// the negative: `ensure_user_and_membership` is only ever told about the
/// login's own tenant, so `list_memberships_for_user` for that user must
/// come back with exactly one entry, scoped to A.
#[tokio::test]
async fn tenant_login_creates_membership_only_in_the_login_tenant() {
    let st = multi_tenant_state().await;
    let tenant_a = crate::tenant::TenantId(1);
    let tenant_b = crate::tenant::TenantId(2);
    st.store
        .put_tenant(&crate::tenant::Tenant {
            id: tenant_a,
            name: "Acme".to_string(),
            slug: "acme".to_string(),
            created: 0,
        })
        .await
        .unwrap();
    st.store
        .put_tenant(&crate::tenant::Tenant {
            id: tenant_b,
            name: "Bravo".to_string(),
            slug: "bravo".to_string(),
            created: 0,
        })
        .await
        .unwrap();
    let cfg_a = crate::oidc::TenantOidcConfig {
        tenant_id: tenant_a,
        issuer: "https://idp.acme.example".into(),
        client_id: "acme".into(),
        client_secret: "s".into(),
        scopes: vec!["openid".into()],
        admin_claim: "groups".into(),
        admin_value: "acme-admins".into(),
        readonly_value: "acme-viewers".into(),
        member_value: String::new(),
        required_value: None,
        post_login_url: None,
        post_logout_url: None,
    };

    let claims = serde_json::json!({ "groups": ["acme-admins"] });
    let role = crate::oidc::claim_role(&claims, &cfg_a);
    let uid = crate::oidc::ensure_user_and_membership(
        st.store.as_ref(),
        true,
        "sub-cross-tenant",
        "x@acme.example",
        "X",
        &[],
        Some((tenant_a, role)),
    )
    .await
    .unwrap();

    let memberships = st.store.list_memberships_for_user(uid).await.unwrap();
    assert_eq!(
        memberships.len(),
        1,
        "the login into A must not create a membership anywhere else"
    );
    assert_eq!(memberships[0].tenant_id, tenant_a);
    assert!(
        st.store
            .get_membership(uid, tenant_b)
            .await
            .unwrap()
            .is_none(),
        "no membership must exist in tenant B from a login into tenant A"
    );
}

/// Security fix (P2d Task 5b, final-branch-review finding): a tenant's
/// `Owner` logging back in through that tenant's own IdP must not be
/// downgraded by whatever role the claim maps to. `claim_role` never
/// produces `Owner`, so before this fix a second login by the sole Owner
/// silently demoted them and left the tenant with no Owner at all
/// (Owner-only operations become unreachable — an availability bug, not
/// an escalation). `ensure_user_and_membership` must now read the
/// existing membership first and keep `Owner` rather than overwrite it.
#[tokio::test]
async fn tenant_login_never_downgrades_an_existing_owner() {
    let st = multi_tenant_state().await;
    let tenant = crate::tenant::TenantId(1);
    st.store
        .put_tenant(&crate::tenant::Tenant {
            id: tenant,
            name: "Acme".to_string(),
            slug: "acme".to_string(),
            created: 0,
        })
        .await
        .unwrap();

    // Grant Owner the way a real workspace creation would (never through
    // `ensure_user_and_membership`/claim_role, which can't produce it).
    let uid = st.store.next_user_id().await.unwrap();
    st.store
        .put_user(&crate::tenant::User {
            id: uid,
            subject: "sub-owner".to_string(),
            email: "owner@acme.example".to_string(),
            display: "Owner".to_string(),
            created: 0,
        })
        .await
        .unwrap();
    st.store
        .put_membership(&crate::tenant::Membership {
            user_id: uid,
            tenant_id: tenant,
            role: crate::tenant::Role::Owner,
            created: 0,
        })
        .await
        .unwrap();

    // The IdP's admin-group claim maps to Admin, not Owner — as if the
    // Owner were removed from the admin group, or the tenant's IdP
    // config simply doesn't distinguish an Owner group at all.
    let cfg = crate::oidc::TenantOidcConfig {
        tenant_id: tenant,
        issuer: "https://idp.acme.example".into(),
        client_id: "acme".into(),
        client_secret: "s".into(),
        scopes: vec!["openid".into()],
        admin_claim: "groups".into(),
        admin_value: "acme-admins".into(),
        readonly_value: "acme-viewers".into(),
        member_value: String::new(),
        required_value: None,
        post_login_url: None,
        post_logout_url: None,
    };
    let claims = serde_json::json!({ "groups": ["acme-admins"] });
    let role = crate::oidc::claim_role(&claims, &cfg);
    assert_eq!(role, crate::tenant::Role::Admin);

    let uid2 = crate::oidc::ensure_user_and_membership(
        st.store.as_ref(),
        true,
        "sub-owner",
        "owner@acme.example",
        "Owner",
        &[],
        Some((tenant, role)),
    )
    .await
    .unwrap();
    assert_eq!(uid2, uid, "same subject must resolve to the same user");

    let m = st.store.get_membership(uid, tenant).await.unwrap().unwrap();
    assert_eq!(
        m.role,
        crate::tenant::Role::Owner,
        "the Owner's own login must not downgrade them via the claim"
    );
}

/// Counterpart to the Owner-preservation test above: a non-owner's
/// membership must still follow the claim on every login, so a group
/// change (Member promoted into the admin group) keeps taking effect.
/// Only `Owner` is special-cased; this asserts the fix didn't freeze
/// every role.
#[tokio::test]
async fn tenant_login_still_applies_claim_role_for_non_owners() {
    let st = multi_tenant_state().await;
    let tenant = crate::tenant::TenantId(1);
    st.store
        .put_tenant(&crate::tenant::Tenant {
            id: tenant,
            name: "Acme".to_string(),
            slug: "acme".to_string(),
            created: 0,
        })
        .await
        .unwrap();
    let cfg = crate::oidc::TenantOidcConfig {
        tenant_id: tenant,
        issuer: "https://idp.acme.example".into(),
        client_id: "acme".into(),
        client_secret: "s".into(),
        scopes: vec!["openid".into()],
        admin_claim: "groups".into(),
        admin_value: "acme-admins".into(),
        readonly_value: "acme-viewers".into(),
        member_value: String::new(),
        required_value: None,
        post_login_url: None,
        post_logout_url: None,
    };

    // First login: no admin-group claim yet, lands as Member (default).
    let member_claims = serde_json::json!({ "groups": ["nobody"] });
    let role = crate::oidc::claim_role(&member_claims, &cfg);
    assert_eq!(role, crate::tenant::Role::Member);
    let uid = crate::oidc::ensure_user_and_membership(
        st.store.as_ref(),
        true,
        "sub-member",
        "member@acme.example",
        "Member",
        &[],
        Some((tenant, role)),
    )
    .await
    .unwrap();
    let m = st.store.get_membership(uid, tenant).await.unwrap().unwrap();
    assert_eq!(m.role, crate::tenant::Role::Member);

    // Second login: now in the admin group — must be upgraded to Admin,
    // since a non-owner's role always tracks the claim.
    let admin_claims = serde_json::json!({ "groups": ["acme-admins"] });
    let role = crate::oidc::claim_role(&admin_claims, &cfg);
    assert_eq!(role, crate::tenant::Role::Admin);
    let uid2 = crate::oidc::ensure_user_and_membership(
        st.store.as_ref(),
        true,
        "sub-member",
        "member@acme.example",
        "Member",
        &[],
        Some((tenant, role)),
    )
    .await
    .unwrap();
    assert_eq!(uid2, uid);
    let m = st.store.get_membership(uid, tenant).await.unwrap().unwrap();
    assert_eq!(
        m.role,
        crate::tenant::Role::Admin,
        "a non-owner's role must follow the claim on every login"
    );
}

/// Brand-new user via per-tenant login still just gets the claim role —
/// the Owner-preservation branch in `ensure_user_and_membership` must not
/// change behavior when there is no prior membership to preserve.
#[tokio::test]
async fn tenant_login_new_user_gets_claim_role() {
    let st = multi_tenant_state().await;
    let tenant = crate::tenant::TenantId(1);
    st.store
        .put_tenant(&crate::tenant::Tenant {
            id: tenant,
            name: "Acme".to_string(),
            slug: "acme".to_string(),
            created: 0,
        })
        .await
        .unwrap();
    let cfg = crate::oidc::TenantOidcConfig {
        tenant_id: tenant,
        issuer: "https://idp.acme.example".into(),
        client_id: "acme".into(),
        client_secret: "s".into(),
        scopes: vec!["openid".into()],
        admin_claim: "groups".into(),
        admin_value: "acme-admins".into(),
        readonly_value: "acme-viewers".into(),
        member_value: String::new(),
        required_value: None,
        post_login_url: None,
        post_logout_url: None,
    };
    let claims = serde_json::json!({ "groups": ["acme-admins"] });
    let role = crate::oidc::claim_role(&claims, &cfg);
    let uid = crate::oidc::ensure_user_and_membership(
        st.store.as_ref(),
        true,
        "sub-brand-new",
        "new@acme.example",
        "New",
        &[],
        Some((tenant, role)),
    )
    .await
    .unwrap();
    let m = st.store.get_membership(uid, tenant).await.unwrap().unwrap();
    assert_eq!(m.role, crate::tenant::Role::Admin);
}

/// Required-group gate (multi-tenancy P2d Task 4b), driven at the same
/// level as `tenant_login_membership_role_matches_claim_mapping`:
/// `passes_required_group` is the decision `oidc_callback` must check
/// BEFORE `ensure_user_and_membership`, so this asserts the gate denies
/// (and nothing is written) rather than merely returning `false`.
/// Without `required_value` set, the gate stays open — unchanged from
/// before this task.
#[tokio::test]
async fn required_group_gate_open_when_unconfigured() {
    let st = multi_tenant_state().await;
    let cfg = crate::oidc::TenantOidcConfig {
        tenant_id: crate::tenant::TenantId(1),
        issuer: "https://idp.acme.example".into(),
        client_id: "acme".into(),
        client_secret: "s".into(),
        scopes: vec!["openid".into()],
        admin_claim: "groups".into(),
        admin_value: "acme-admins".into(),
        readonly_value: "acme-viewers".into(),
        member_value: String::new(),
        required_value: None,
        post_login_url: None,
        post_logout_url: None,
    };
    let tenant = crate::tenant::TenantId(1);

    // Any authenticated user, in none of the groups, still passes the
    // gate (though `claim_role` still gives them only the open Member
    // default) — the open-by-default contract this task must not break.
    let claims = serde_json::json!({ "groups": ["nobody-in-particular"] });
    assert!(crate::oidc::passes_required_group(&claims, &cfg));
    let role = crate::oidc::claim_role(&claims, &cfg);
    assert_eq!(role, crate::tenant::Role::Member);
    let uid = crate::oidc::ensure_user_and_membership(
        st.store.as_ref(),
        true,
        "sub-open",
        "open@acme.example",
        "Open",
        &[],
        Some((tenant, role)),
    )
    .await
    .unwrap();
    let m = st.store.get_membership(uid, tenant).await.unwrap().unwrap();
    assert_eq!(m.role, crate::tenant::Role::Member);
}

/// With `required_value` set: a user in none of admin/readonly/required
/// is denied by the gate BEFORE any membership is considered; a member of
/// the required group passes (and gets the open Member role, since they
/// match neither admin_value nor readonly_value); a member of the admin
/// group passes the gate too (their claim already satisfies it) and
/// keeps the Admin role.
#[tokio::test]
async fn required_group_gate_closed_when_configured() {
    let st = multi_tenant_state().await;
    let cfg = crate::oidc::TenantOidcConfig {
        tenant_id: crate::tenant::TenantId(1),
        issuer: "https://idp.acme.example".into(),
        client_id: "acme".into(),
        client_secret: "s".into(),
        scopes: vec!["openid".into()],
        admin_claim: "groups".into(),
        admin_value: "acme-admins".into(),
        readonly_value: "acme-viewers".into(),
        member_value: String::new(),
        required_value: Some("acme-contractors".to_string()),
        post_login_url: None,
        post_logout_url: None,
    };
    let tenant = crate::tenant::TenantId(1);

    // Neither admin, readonly, nor the required group: the gate denies,
    // and (mirroring exactly what `oidc_callback` does on this branch)
    // `ensure_user_and_membership` is never reached — no user, no
    // membership, for an outsider who never should have gotten in.
    let outsider_claims = serde_json::json!({ "groups": ["random"] });
    assert!(!crate::oidc::passes_required_group(&outsider_claims, &cfg));
    assert!(
        st.store
            .get_user_by_subject("sub-outsider")
            .await
            .unwrap()
            .is_none(),
        "a caller denied by the required-group gate must never get a user record"
    );

    // The required group itself: gate passes, and (since they match
    // neither admin_value nor readonly_value) `claim_role` still gives
    // them only Member — the gate and the role are independent checks.
    let required_claims = serde_json::json!({ "groups": ["acme-contractors"] });
    assert!(crate::oidc::passes_required_group(&required_claims, &cfg));
    let role = crate::oidc::claim_role(&required_claims, &cfg);
    assert_eq!(role, crate::tenant::Role::Member);
    let uid = crate::oidc::ensure_user_and_membership(
        st.store.as_ref(),
        true,
        "sub-required",
        "required@acme.example",
        "Required",
        &[],
        Some((tenant, role)),
    )
    .await
    .unwrap();
    let m = st.store.get_membership(uid, tenant).await.unwrap().unwrap();
    assert_eq!(m.role, crate::tenant::Role::Member);

    // The admin group: gate passes (their claim already satisfies it
    // independent of `required_value`), and the role is still Admin.
    let admin_claims = serde_json::json!({ "groups": ["acme-admins"] });
    assert!(crate::oidc::passes_required_group(&admin_claims, &cfg));
    let admin_role = crate::oidc::claim_role(&admin_claims, &cfg);
    assert_eq!(admin_role, crate::tenant::Role::Admin);
    let admin_uid = crate::oidc::ensure_user_and_membership(
        st.store.as_ref(),
        true,
        "sub-admin",
        "admin@acme.example",
        "Admin",
        &[],
        Some((tenant, admin_role)),
    )
    .await
    .unwrap();
    let admin_m = st
        .store
        .get_membership(admin_uid, tenant)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(admin_m.role, crate::tenant::Role::Admin);
}

/// `admin_guard` returns the resolved `Principal` on every success path
/// while keeping the status contract (401/403) byte for byte. The
/// integration admin-auth tests guard the full 401/403/404/429/503 matrix
/// end to end; this asserts the in-process Principal contents the HTTP
/// surface cannot observe in P1b (tenant is always the default).
#[tokio::test]
async fn admin_guard_resolves_principal_per_credential() {
    use super::admin_guard;
    use crate::auth::{hash_token, ApiToken, Scope};
    use crate::tenant::DEFAULT_TENANT;
    use axum::http::{HeaderMap as GuardHeaders, StatusCode};

    let st = guard_state(Some("secret")).await;

    // 1) env admin token present + provided -> Full principal, default tenant.
    let mut h = GuardHeaders::new();
    h.insert(HEADER_ADMIN_TOKEN, "secret".parse().unwrap());
    let p = admin_guard(&st, &h, Scope::Full)
        .await
        .expect("env admin token authorizes");
    assert_eq!(p.tenant, DEFAULT_TENANT);
    assert_eq!(p.user_id, None);
    assert_eq!(p.scopes, vec![Scope::Full]);

    // 3) no credential, env token configured -> 401 (contract preserved).
    assert_eq!(
        admin_guard(&st, &GuardHeaders::new(), Scope::Full)
            .await
            .unwrap_err(),
        StatusCode::UNAUTHORIZED
    );

    // A stored API token scoped to [LinksRead] on the default tenant.
    let plaintext = "qtok_principal_resolution_test";
    let token = ApiToken {
        id: 1,
        name: "t".into(),
        token_hash: hash_token(plaintext),
        scopes: vec![Scope::LinksRead],
        rate_limit_per_min: None,
        created: 0,
        tenant_id: DEFAULT_TENANT,
    };
    st.store
        .put_api_token(DEFAULT_TENANT, &token)
        .await
        .unwrap();
    let mut ht = GuardHeaders::new();
    ht.insert(HEADER_ADMIN_TOKEN, plaintext.parse().unwrap());

    // 2) covering API token -> Principal carries the token's tenant + scopes.
    let p = admin_guard(&st, &ht, Scope::LinksRead)
        .await
        .expect("api token covers LinksRead");
    assert_eq!(p.tenant, DEFAULT_TENANT);
    assert_eq!(p.user_id, None);
    assert_eq!(p.scopes, vec![Scope::LinksRead]);

    // 4) valid-but-insufficient token -> 403 (contract preserved).
    assert_eq!(
        admin_guard(&st, &ht, Scope::Full).await.unwrap_err(),
        StatusCode::FORBIDDEN
    );
}

/// OSS session with EMPTY `session.scopes` must still yield 403, not 401.
/// The OIDC-session branch in `admin_guard` unconditionally sets
/// `saw_insufficient` after a failed covering check (byte-for-byte with
/// the original behavior) precisely so this case falls through to the
/// 403 tail instead of `not_found_status` (401). The OIDC callback
/// currently rejects empty-scope logins, so this session shape doesn't
/// arise in practice today — but the guard's own status contract must
/// not depend on that invariant holding in another function.
#[tokio::test]
async fn admin_guard_oss_empty_scope_session_is_forbidden_not_unauthorized() {
    use super::admin_guard;
    use crate::auth::{hash_token, Scope, Session};
    use axum::http::{HeaderMap as GuardHeaders, StatusCode};

    let st = guard_state_with_oidc(None, true).await;
    assert!(!st.multi_tenant);

    let raw = "oss_empty_scope_session_test";
    let session = Session {
        token_hash: hash_token(raw),
        subject: "sub".into(),
        display: "display".into(),
        scopes: Vec::new(),
        created: 0,
        expires: u64::MAX,
        tenant_id: crate::tenant::DEFAULT_TENANT,
        user_id: 7,
        id_token: None,
    };
    st.store
        .put_session(crate::tenant::DEFAULT_TENANT, &session)
        .await
        .unwrap();

    let mut headers = GuardHeaders::new();
    headers.insert(
        axum::http::header::COOKIE,
        format!("qk_session={raw}").parse().unwrap(),
    );

    assert_eq!(
        admin_guard(&st, &headers, Scope::LinksRead)
            .await
            .unwrap_err(),
        StatusCode::FORBIDDEN
    );
}

/// `session_user_id` must gate on `st.oidc_configured`, same as
/// `admin_guard`'s session branch: a leftover session cookie must stop
/// resolving a user the instant OIDC is disabled, even though the
/// session row itself is still valid in the store.
#[tokio::test]
async fn session_user_id_none_when_oidc_not_configured() {
    use super::session_user_id;
    use crate::auth::{hash_token, Session};

    let raw = "session_gate_test_token";
    let session = Session {
        token_hash: hash_token(raw),
        subject: "sub".into(),
        display: "display".into(),
        scopes: Vec::new(),
        created: 0,
        expires: u64::MAX,
        tenant_id: crate::tenant::DEFAULT_TENANT,
        user_id: 42,
        id_token: None,
    };

    // OIDC disabled: even a store-valid session cookie must resolve to
    // nothing, matching admin_guard's session-branch gate.
    let st_off = guard_state_with_oidc(None, false).await;
    st_off
        .store
        .put_session(crate::tenant::DEFAULT_TENANT, &session)
        .await
        .unwrap();
    let mut headers = ReqHeaderMap::new();
    headers.insert(
        axum::http::header::COOKIE,
        format!("qk_session={raw}").parse().unwrap(),
    );
    assert_eq!(session_user_id(&st_off, &headers).await, None);

    // OIDC enabled + same valid session -> resolves the user_id.
    let st_on = guard_state_with_oidc(None, true).await;
    st_on
        .store
        .put_session(crate::tenant::DEFAULT_TENANT, &session)
        .await
        .unwrap();
    assert_eq!(session_user_id(&st_on, &headers).await, Some(42));
}

/// P2a Task 3: `create_link_core` must write under the `tenant` PARAM, not
/// `DEFAULT_TENANT`. Exercises both branches (numeric id and custom alias)
/// against a store keyed by tenant, so a regression to the old hardcode
/// would make the link/alias invisible under the passed tenant (and
/// visible under `DEFAULT_TENANT` instead).
#[tokio::test]
async fn create_link_core_numeric_writes_under_the_passed_tenant() {
    let st = guard_state(None).await;
    let tenant = crate::tenant::TenantId(7);
    let headers = ReqHeaderMap::new();

    let code = create_link_core(
        &st,
        tenant,
        SHARED_DOMAIN_ID,
        "https://example.com/numeric",
        None,
        None,
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        None,
        None,
        None,
        None,
        None,
        &headers,
    )
    .await
    .expect("create succeeds");
    let permuted = crate::codec::from_base62(&code).expect("numeric code decodes");
    let id = crate::permute::decode(permuted, st.key);

    assert!(
        st.store.get_link(tenant, id).await.unwrap().is_some(),
        "the link must be visible under the passed tenant"
    );
    assert!(
        st.store
            .get_link(crate::tenant::DEFAULT_TENANT, id)
            .await
            .unwrap()
            .is_none(),
        "the link must NOT be visible under DEFAULT_TENANT"
    );
}

/// P3 Task 2: the alias namespace moved from per-tenant to per-domain, so
/// a written alias now resolves regardless of which tenant asks (any
/// tenant creating through the shared namespace lands on the same
/// `SHARED_DOMAIN_ID`). Cross-domain isolation itself is exercised by the
/// PG-gated `alias_namespace_is_per_domain` in `tests/domains_it.rs`.
#[tokio::test]
async fn create_link_core_alias_resolves_via_the_shared_domain() {
    let st = guard_state(None).await;
    let tenant = crate::tenant::TenantId(7);
    let headers = ReqHeaderMap::new();

    let code = create_link_core(
        &st,
        tenant,
        SHARED_DOMAIN_ID,
        "https://example.com/alias",
        Some("my-alias"),
        None,
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        None,
        None,
        None,
        None,
        None,
        &headers,
    )
    .await
    .expect("create succeeds");
    assert_eq!(code, "my-alias");

    assert!(
        st.store
            .get_alias(SHARED_DOMAIN_ID, "my-alias")
            .await
            .unwrap()
            .is_some(),
        "the alias must resolve in the shared domain namespace"
    );
}

/// `resolve_code`'s alias branch resolves through whichever domain id is
/// passed in; the shared domain (`SHARED_DOMAIN_ID`) is one such domain,
/// used regardless of which tenant created the alias (alias isolation is
/// by domain, not by tenant; see `resolve_code`'s doc comment).
#[tokio::test]
async fn resolve_code_resolves_alias_via_the_shared_domain() {
    let st = guard_state(None).await;
    let tenant = crate::tenant::TenantId(9);
    let rec = Record {
        url: "https://example.com".into(),
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
    };
    st.store
        .put_alias_and_link(tenant, SHARED_DOMAIN_ID, "foo", 5, &rec)
        .await
        .unwrap();

    assert_eq!(
        resolve_code(&st, SHARED_DOMAIN_ID, "foo").await.unwrap(),
        Some(5),
        "resolve_code must resolve the alias via the shared domain"
    );
}

/// `resolve_for_admin`'s alias branch resolves through the shared domain
/// namespace, mirroring `resolve_code`.
#[tokio::test]
async fn resolve_for_admin_resolves_alias_via_the_shared_domain() {
    let st = guard_state(None).await;
    let tenant = crate::tenant::TenantId(11);
    let rec = Record {
        url: "https://example.com".into(),
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
    };
    st.store
        .put_alias_and_link(tenant, SHARED_DOMAIN_ID, "bar", 6, &rec)
        .await
        .unwrap();

    assert_eq!(
        resolve_for_admin(&st, tenant, "bar").await.unwrap(),
        Some((6, Some("bar".to_string()))),
        "resolve_for_admin must resolve the alias via the shared domain"
    );
    assert_eq!(
        resolve_for_admin(&st, crate::tenant::DEFAULT_TENANT, "bar")
            .await
            .unwrap(),
        Some((6, Some("bar".to_string()))),
        "resolve_for_admin must resolve the alias regardless of the passed tenant"
    );
}

const IPHONE_UA: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X)";
const IPAD_UA: &str = "Mozilla/5.0 (iPad; CPU OS 17_0 like Mac OS X)";
const IPOD_UA: &str = "Mozilla/5.0 (iPod touch; CPU iPhone OS 17_0 like Mac OS X)";
const ANDROID_UA: &str = "Mozilla/5.0 (Linux; Android 14; Pixel 8)";
const DESKTOP_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64)";

#[test]
fn normalize_max_visits_zero_or_absent_is_unlimited() {
    assert_eq!(normalize_max_visits(None), None);
    assert_eq!(normalize_max_visits(Some(0)), None);
}

#[test]
fn normalize_max_visits_positive_is_some() {
    assert_eq!(normalize_max_visits(Some(1)), Some(1));
    assert_eq!(normalize_max_visits(Some(42)), Some(42));
}

#[test]
fn fbclid_from_query_present() {
    assert_eq!(
        fbclid_from_query(Some("a=1&fbclid=abc123&b=2")),
        Some("abc123".to_string())
    );
}

#[test]
fn fbclid_from_query_absent() {
    assert_eq!(fbclid_from_query(Some("a=1&b=2")), None);
    assert_eq!(fbclid_from_query(None), None);
}

#[test]
fn fbclid_from_query_urlencoded_value_is_decoded() {
    assert_eq!(
        fbclid_from_query(Some("fbclid=IwAR%2Bx%20y")),
        Some("IwAR+x y".to_string())
    );
}

#[test]
fn fbclid_from_query_empty_is_none() {
    assert_eq!(fbclid_from_query(Some("")), None);
    assert_eq!(fbclid_from_query(Some("fbclid=")), None);
}

#[test]
fn classify_platform_detects_apple_devices() {
    assert_eq!(classify_platform(Some(IPHONE_UA)), Platform::Ios);
    assert_eq!(classify_platform(Some(IPAD_UA)), Platform::Ios);
    assert_eq!(classify_platform(Some(IPOD_UA)), Platform::Ios);
}

#[test]
fn classify_platform_detects_android() {
    assert_eq!(classify_platform(Some(ANDROID_UA)), Platform::Android);
}

#[test]
fn classify_platform_falls_back_to_other() {
    assert_eq!(classify_platform(Some(DESKTOP_UA)), Platform::Other);
    assert_eq!(classify_platform(Some("")), Platform::Other);
    assert_eq!(classify_platform(None), Platform::Other);
}

#[test]
fn app_destination_returns_platform_match() {
    let r = rec(
        Some("https://apps.apple.com/x"),
        Some("https://play.google.com/y"),
    );
    assert_eq!(
        app_destination(&r, Some(IPHONE_UA)),
        Some("https://apps.apple.com/x")
    );
    assert_eq!(
        app_destination(&r, Some(ANDROID_UA)),
        Some("https://play.google.com/y")
    );
}

#[test]
fn app_destination_falls_back_when_platform_unset() {
    let r = rec(Some("https://apps.apple.com/x"), None);
    assert_eq!(app_destination(&r, Some(ANDROID_UA)), None);
    assert_eq!(app_destination(&r, Some(DESKTOP_UA)), None);
}

#[test]
fn app_destination_none_when_no_fields() {
    let r = rec(None, None);
    assert_eq!(app_destination(&r, Some(IPHONE_UA)), None);
    assert_eq!(app_destination(&r, Some(ANDROID_UA)), None);
}

#[test]
fn parse_cors_origins_splits_and_trims() {
    assert_eq!(parse_cors_origins(None), Vec::<String>::new());
    assert_eq!(parse_cors_origins(Some("".into())), Vec::<String>::new());
    assert_eq!(
        parse_cors_origins(Some(" https://a.com , https://b.com ".into())),
        vec!["https://a.com".to_string(), "https://b.com".to_string()]
    );
}

#[test]
fn cache_control_without_expiry_uses_default() {
    assert_eq!(cache_control_for(None, 1_000), "public, max-age=86400");
}

#[test]
fn cache_control_with_future_expiry_uses_difference() {
    let now = 1_000;
    assert_eq!(
        cache_control_for(Some(now + 100), now),
        "public, max-age=100"
    );
}

#[test]
fn cache_control_with_distant_future_expiry_caps_at_default() {
    let now = 1_000;
    assert_eq!(
        cache_control_for(Some(now + 999_999), now),
        "public, max-age=86400"
    );
}

#[test]
fn cache_control_with_past_expiry_is_no_store() {
    let now = 1_000;
    assert_eq!(cache_control_for(Some(now - 1), now), "no-store");
}

#[test]
fn access_log_line_is_valid_json_with_expected_fields() {
    let line = access_log_line("GET", "/abc", 302, 0.4139);
    let v: serde_json::Value =
        serde_json::from_str(&line).expect("access_log_line should produce valid JSON");
    assert_eq!(v["method"], "GET");
    assert_eq!(v["path"], "/abc");
    assert_eq!(v["status"], 302);
    assert_eq!(v["latency_ms"], 0.414);
}

#[test]
fn access_log_line_escapes_special_characters_in_path() {
    let path = "/a\"b\\c";
    let line = access_log_line("GET", path, 200, 1.0);
    let v: serde_json::Value = serde_json::from_str(&line)
        .expect("access_log_line should escape correctly and remain valid JSON");
    assert_eq!(v["path"], path);
}

#[test]
fn normalize_admin_host_strips_port_dot_case() {
    use super::normalize_admin_host;
    // Host header comparison must ignore port, trailing dot, and case, so the
    // admin-host gate matches `backend.quarkus.com.br` regardless of how the
    // client formats the Host header.
    assert_eq!(normalize_admin_host("Backend.Quarkus.COM.br"), "backend.quarkus.com.br");
    assert_eq!(normalize_admin_host("backend.quarkus.com.br:443"), "backend.quarkus.com.br");
    assert_eq!(normalize_admin_host("backend.quarkus.com.br."), "backend.quarkus.com.br");
    assert_eq!(normalize_admin_host("  backend.quarkus.com.br  "), "backend.quarkus.com.br");
    // A tenant link domain normalizes to itself (never equals the admin host).
    assert_ne!(normalize_admin_host("go.meuchat.ai"), "backend.quarkus.com.br");
}

/// Captured request: headers (lowercased names) + raw body. Mirrors the
/// mock server in `webhooks::delivery`'s test module.
struct Captured {
    headers: std::collections::HashMap<String, String>,
    body: String,
}

struct ServerState {
    captured: Mutex<Vec<Captured>>,
}

async fn handler(
    State(state): State<std::sync::Arc<ServerState>>,
    headers: ReqHeaderMap,
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
    axum::http::StatusCode::OK
}

/// Spins a local server capturing every POST it receives. Returns the
/// base URL and the shared state to inspect.
async fn spawn_test_server() -> (String, std::sync::Arc<ServerState>) {
    let state = std::sync::Arc::new(ServerState {
        captured: Mutex::new(Vec::new()),
    });
    let app = TestRouter::new()
        .route("/hook", any(handler))
        .with_state(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}/hook"), state)
}

fn sub(url: &str, secret: &str, kind: SubscriptionKind) -> WebhookSubscription {
    WebhookSubscription {
        id: 1,
        url: url.to_string(),
        events: vec![EventType::LinkCreated],
        secret: secret.to_string(),
        active: true,
        created: 0,
        kind,
    }
}

/// Regression for review Task 1 of #6: a Slack-kind subscription's
/// test-send must receive the same channel-formatted, unsigned payload a
/// real delivery would send — not the signed Generic envelope the
/// endpoint used to always build. This is exercised through
/// `send_test_event_guarded` (the SSRF-guard-injectable core of
/// `admin_webhooks_test`) since the guard's real predicate always blocks
/// the loopback address a local test server binds to (see that
/// function's doc comment).
#[tokio::test]
async fn test_send_on_slack_sub_is_unsigned_channel_payload() {
    let (url, state) = spawn_test_server().await;
    let slack_sub = sub(&url, "", SubscriptionKind::Slack);

    let resp = send_test_event_guarded(&slack_sub, |_| false).await;
    assert_eq!(resp.status(), axum::http::StatusCode::OK);

    let captured = state.captured.lock().unwrap();
    assert_eq!(captured.len(), 1);
    let req = &captured[0];
    let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
    assert!(body["text"].as_str().unwrap().contains("TEST0000"));
    assert!(!req.headers.contains_key("webhook-signature"));
    assert!(!req.headers.contains_key("webhook-id"));
    assert!(!req.headers.contains_key("webhook-timestamp"));
}

/// Counterpart: a Generic subscription's test-send must remain the
/// signed Standard Webhooks envelope, body verbatim.
#[tokio::test]
async fn test_send_on_generic_sub_stays_signed() {
    let (url, state) = spawn_test_server().await;
    let secret = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw".to_string();
    let generic_sub = sub(&url, &secret, SubscriptionKind::Generic);

    let resp = send_test_event_guarded(&generic_sub, |_| false).await;
    assert_eq!(resp.status(), axum::http::StatusCode::OK);

    let captured = state.captured.lock().unwrap();
    assert_eq!(captured.len(), 1);
    let req = &captured[0];
    let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
    assert_eq!(body["data"]["code"], "TEST0000");
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
    let expected = crate::webhooks::sign(&secret, msg_id, ts, &req.body).unwrap();
    assert_eq!(sig, &expected);
}
