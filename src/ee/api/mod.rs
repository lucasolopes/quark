//! Enterprise HTTP routes. Covered by `src/ee/LICENSE`, not by the AGPL.
//!
//! The 15 routes mounted here are exactly the ones that already answered 404
//! when `!st.multi_tenant`: administering workspaces, inviting other people,
//! configuring a per-tenant IdP, SSO discovery by email domain, and custom
//! domains with DNS verification. The inventory that classified each one is in
//! `docs/research/2026-08-03-luc19-inventario-oss-ee.md`.

// The core's internal namespace (`AppState`, `admin_guard`, `client_ip`, the
// domain types, the axum prelude). Re-exported so the submodules here reach all
// of it through a plain `use super::*`, the same way `src/api/`'s own
// submodules do.
pub(crate) use crate::api::*;

mod domains;
pub mod entitlement;
mod invites;
mod sso_domains;
pub(crate) mod tenant_idp;
mod tenants;

pub(crate) use domains::*;
pub(crate) use invites::*;
pub(crate) use sso_domains::*;
pub use tenants::*;

/// Injection point called by `crate::api::router_with_cors`. Takes the core
/// router and returns it with the Enterprise routes mounted.
pub fn mount(r: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    r.route("/admin/tenants", post(admin_tenants_create))
        .route(
            "/admin/tenants/{id}",
            axum::routing::delete(admin_tenants_delete),
        )
        .route("/admin/workspace/switch", post(admin_workspace_switch))
        .route(
            "/admin/oidc-config",
            get(admin_oidc_config_get)
                .put(admin_oidc_config_put)
                .delete(admin_oidc_config_delete),
        )
        .route(
            "/admin/domains",
            get(admin_domains_list).post(admin_domains_create),
        )
        .route(
            "/admin/domains/{id}",
            axum::routing::delete(admin_domains_delete),
        )
        .route("/admin/domains/{id}/verify", post(admin_domains_verify))
        .route(
            "/admin/domains/{id}/primary",
            post(admin_domains_set_primary),
        )
        .route(
            "/admin/sso-domains",
            get(admin_sso_domains_list).post(admin_sso_domains_create),
        )
        .route(
            "/admin/sso-domains/{id}",
            axum::routing::delete(admin_sso_domains_delete),
        )
        .route(
            "/admin/sso-domains/{id}/verify",
            post(admin_sso_domains_verify),
        )
        .route("/admin/sso/discover", get(sso_discover))
        .route(
            "/admin/invites",
            get(admin_invites_list).post(admin_invites_create),
        )
        .route(
            "/admin/invites/{id}",
            axum::routing::delete(admin_invites_delete),
        )
        .route("/admin/invites/{token}/accept", post(admin_invites_accept))
}

// Session helpers used only by the Enterprise handlers. They lived in
// `api/oidc_login.rs` until LUC-19; each one's doc names the EE handler that
// calls it (`/admin/tenants`, `admin_tenants_delete`, `put_tenant`). They moved
// along so the Community build carries no dead code.

/// Resolves the session cookie to its `user_id`, independent of scopes. Used
/// by `/admin/tenants`: creating a first workspace must be reachable by ANY
/// authenticated OIDC user, including one with zero memberships (so
/// `admin_guard`'s scope check, which a 0-membership user could never pass,
/// does not apply here). Gated on `st.oidc_configured`, same as `admin_guard`'s
/// session branch, so disabling OIDC immediately stops leftover session
/// cookies from resolving a user here too.
pub(super) async fn session_user_id(st: &AppState, headers: &HeaderMap) -> Option<u64> {
    Some(current_session(st, headers).await?.user_id)
}

/// The whole session row behind the request's cookie, on the same terms
/// `session_user_id` resolves the user id (it is the thin wrapper over this).
/// Callers that need more than the user id use this: `admin_tenants_delete`
/// needs the session's CURRENT workspace, to know whether the workspace being
/// deleted is the one the caller is sitting in, and needs the row itself
/// captured before the delete, since `sessions` is tenant-owned and the row
/// goes down with the tenant.
pub(super) async fn current_session(
    st: &AppState,
    headers: &HeaderMap,
) -> Option<crate::auth::Session> {
    if !st.oidc_configured {
        return None;
    }
    let raw = crate::api::cookie_value(headers, crate::api::SESSION_COOKIE)?;
    st.store
        .get_session_by_hash(&hash_token(raw), now())
        .await
        .ok()
        .flatten()
}

/// Re-points the current session at `tenant` (the workspace just created),
/// so the next request the browser makes is already scoped to it. A missing
/// or invalid session is a silent no-op: the caller already authenticated via
/// `session_user_id` earlier in the same request.
pub(super) async fn set_session_tenant(
    st: &AppState,
    headers: &HeaderMap,
    tenant: crate::tenant::TenantId,
) {
    let Some(raw) = crate::api::cookie_value(headers, crate::api::SESSION_COOKIE) else {
        return;
    };
    let hash = hash_token(raw);
    if let Ok(Some(mut session)) = st.store.get_session_by_hash(&hash, now()).await {
        session.tenant_id = tenant;
        let _ = st.store.put_session(tenant, &session).await;
    }
}

/// Maps a `put_tenant` failure to its HTTP status: a duplicate `slug` (unique
/// violation) is a client-fixable `409`, anything else is a `503` (backend
/// unavailable).
pub(super) fn conflict_or_503(e: StoreError) -> StatusCode {
    match e {
        StoreError::UniqueViolation => StatusCode::CONFLICT,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    }
}

#[cfg(test)]
mod tests {
    //! Moved out of `src/api/tests.rs` in LUC-19, along with the helper it
    //! exercises.

    /// `session_user_id` must gate on `st.oidc_configured`, same as
    /// `admin_guard`'s session branch: a leftover session cookie must stop
    /// resolving a user the instant OIDC is disabled, even though the
    /// session row itself is still valid in the store.
    #[tokio::test]
    async fn session_user_id_none_when_oidc_not_configured() {
        use super::session_user_id;
        use crate::api::tests::guard_state_with_oidc;
        use crate::auth::{hash_token, Session};
        use axum::http::HeaderMap as ReqHeaderMap;

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
}
