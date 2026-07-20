use super::*;

/// The authenticated caller behind an authorized admin request: which tenant
/// its data access is scoped to, the backing user (when known — OIDC only),
/// and the scopes the credential carries. Resolved by `admin_guard` and
/// threaded into handlers so tenant-owned reads/writes go through the
/// principal's tenant rather than a hardcoded default.
#[derive(Debug)]
pub struct Principal {
    pub tenant: crate::tenant::TenantId,
    pub user_id: Option<u64>,
    pub scopes: Vec<Scope>,
}

/// Authorizes an admin request against a required `Scope`: `Ok(Principal)` if
/// authorized; `Err(status)` otherwise. Returns `StatusCode` (not `Response`)
/// in the error to stay `Copy`/small — avoids clippy's `result_large_err`
/// lint, which would trigger with `Response` in the `Err`.
///
/// Order (exact status contract, must not regress the env-token-only path):
/// 1. The env `QUARK_ADMIN_TOKEN`, compared in constant time, is always
///    `Full` (superuser) and never touches the store — this preserves the
///    pre-existing behavior byte for byte.
/// 2. Otherwise, a non-empty provided token is hashed and looked up as an
///    API token: found + scope covers `required` + (no per-token rate limit,
///    or under it) -> `Ok`; found but insufficient scope -> `403`; found but
///    over its rate limit -> `429`; not found -> `401` if an env token is
///    configured, else `404` (endpoint fully disabled when nothing is set up).
/// 3. An empty provided token (nothing supplied) follows the same not-found
///    rule: `401` if an env token is configured, else `404`.
pub(crate) async fn admin_guard(
    st: &AppState,
    headers: &HeaderMap,
    required: Scope,
) -> Result<Principal, StatusCode> {
    let provided = headers
        .get("x-admin-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    // 1) Break-glass env admin token (always Full).
    if let Some(expected) = st.admin_token.as_deref() {
        if constant_time_eq(provided.as_bytes(), expected.as_bytes()) {
            return Ok(Principal {
                tenant: crate::tenant::DEFAULT_TENANT,
                user_id: None,
                scopes: vec![Scope::Full],
            });
        }
    }

    // Admin surface is "disabled" (404) only when neither a token nor OIDC is
    // configured; otherwise a missing/wrong credential is 401. Keyed on whether
    // OIDC was configured (not on init success) so a failed IdP init still keeps
    // the surface locked.
    let not_found_status = if st.admin_token.is_some() || st.oidc_configured {
        StatusCode::UNAUTHORIZED
    } else {
        StatusCode::NOT_FOUND
    };

    // Try every presented credential; any one that covers `required` authorizes.
    // A valid-but-insufficient credential yields 403, and a covering-but-throttled
    // API token yields 429, but only if NOTHING else covers — so a low-scope or
    // rate-limited API token in localStorage never blocks a sufficient session.
    let mut saw_insufficient = false;
    let mut saw_rate_limited = false;
    // A store error on one credential's lookup must not deny a user who also
    // presents a covering one: record it and 503 only if nothing else authorizes.
    let mut saw_store_error = false;

    // 2) Scoped API token in x-admin-token.
    if !provided.is_empty() {
        let hash = hash_token(provided);
        match st.store.get_api_token_by_hash(&hash).await {
            Ok(Some(token)) => {
                if token.scopes.iter().any(|s| s.covers(required)) {
                    match token.rate_limit_per_min {
                        Some(limit) => {
                            let key = format!("tok:{}", token.id);
                            if st.ratelimiter.check_with_limit(&key, now(), limit).await {
                                return Ok(Principal {
                                    tenant: token.tenant_id,
                                    user_id: None,
                                    scopes: token.scopes.clone(),
                                });
                            }
                            saw_rate_limited = true;
                        }
                        None => {
                            return Ok(Principal {
                                tenant: token.tenant_id,
                                user_id: None,
                                scopes: token.scopes.clone(),
                            })
                        }
                    }
                } else {
                    saw_insufficient = true;
                }
            }
            Ok(None) => {}
            Err(_) => saw_store_error = true,
        }
    }

    // 3) OIDC login session cookie. Only honored when OIDC is configured, so
    // disabling OIDC immediately stops leftover session cookies from
    // authorizing (the session GC only runs while OIDC is on).
    if st.oidc_configured {
        if let Some(raw) = cookie_value(headers, SESSION_COOKIE) {
            let hash = hash_token(raw);
            match st.store.get_session_by_hash(&hash, now()).await {
                Ok(Some(session)) => {
                    // Where the session's authorization comes from differs by
                    // deployment mode. OSS: the stored `session.scopes`, which
                    // is the OIDC group->scope map computed at login. Cloud: the
                    // caller's role in the CURRENT workspace (`session.tenant_id`),
                    // so switching workspaces re-derives scopes from membership and
                    // never trusts a scope set minted for a different tenant. A
                    // cloud session whose user has no membership in the current
                    // tenant is treated as insufficient (403), never authorized.
                    let effective_scopes = if st.multi_tenant {
                        match st
                            .store
                            .get_membership(session.user_id, session.tenant_id)
                            .await
                        {
                            Ok(Some(m)) => crate::tenant::role_scopes(m.role).to_vec(),
                            // No membership in the current tenant -> empty scopes.
                            // The covering check below fails and the unconditional
                            // `saw_insufficient = true` after it yields 403; setting
                            // the flag here too would be a dead assignment.
                            Ok(None) => vec![],
                            Err(_) => {
                                saw_store_error = true;
                                vec![]
                            }
                        }
                    } else {
                        session.scopes.clone()
                    };
                    if effective_scopes.iter().any(|s| s.covers(required)) {
                        return Ok(Principal {
                            tenant: session.tenant_id,
                            user_id: Some(session.user_id),
                            scopes: effective_scopes,
                        });
                    }
                    saw_insufficient = true;
                }
                Ok(None) => {}
                Err(_) => saw_store_error = true,
            }
        }
    }

    if saw_store_error {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    if saw_rate_limited {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    if saw_insufficient {
        return Err(StatusCode::FORBIDDEN);
    }
    Err(not_found_status)
}

/// Cookie-authenticated state-changing requests must carry a custom header the
/// browser cannot attach to a cross-site "simple" request. A request bearing
/// `x-admin-token` or `x-quark-csrf` is either token-authenticated (not
/// cookie-borne, so not CSRF-able) or was preflighted (custom header forces
/// preflight, gated by the CORS allowlist); either proves it is not a forgeable
/// simple POST riding the SameSite=None session cookie. Call after `admin_guard`
/// on simple-POST endpoints (no JSON body / content-type-sniffed body) that
/// would otherwise be reachable cross-site.
pub(crate) fn csrf_guard(headers: &HeaderMap) -> Result<(), StatusCode> {
    if headers.contains_key("x-admin-token") || headers.contains_key("x-quark-csrf") {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}
