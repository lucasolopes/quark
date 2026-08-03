//! quark Enterprise Edition.
//!
//! WARNING: this directory is NOT AGPL. It is covered by the quark Enterprise
//! Edition License in `src/ee/LICENSE`. Everything else in the repository is
//! AGPL-3.0-only. See `docs/LICENSING.md`.
//!
//! The whole module lives behind the `ee` cargo feature, which is not default:
//!
//! ```text
//! cargo build                 # Community edition, none of this compiled in
//! cargo build --features ee   # Enterprise edition
//! ```
//!
//! Deleting `src/ee/` has to leave the core building and its tests passing. CI
//! proves that on every push; the day it stops holding, the split has become
//! decoration. The rule behind the cut is in
//! `docs/specs/2026-08-03-luc19-open-core-design.md`: what one organization
//! uses for itself stays in the core, what only serves operating quark as a
//! service for other people comes here.

pub mod api;
pub mod keycloak;
pub mod plan;

use std::sync::Arc;

/// The `AppState` fields that only exist in the Enterprise edition.
///
/// Deliberately small. A field belongs here only when it names a type that
/// leaves the core along with this directory: without that, `AppState` would
/// not compile with `src/ee/` absent. Fields the core itself reads stay in the
/// core even when only cloud uses them (`tenant_domain_suffix` is read by
/// `api/links.rs`, `oidc_tenants` is a cache over the store, and `host_router`
/// is on the hot path).
// Not `Clone`: it lives inside the `Arc<AppState>` and is never copied.
// `Default` is what the test builders use to stand one up empty.
#[derive(Default)]
pub struct EeState {
    /// Keycloak admin runtime, present only when `QUARK_KEYCLOAK_BASE_URL` is
    /// configured. `None` disables per-tenant realm provisioning entirely.
    pub keycloak: Option<Arc<dyn keycloak::KeycloakAdmin>>,
    /// Base URL Keycloak answers on, kept alongside the runtime so a tenant's
    /// issuer can be derived without re-reading the environment.
    pub keycloak_base_url: Option<String>,
    /// Per-tenant `OidcRuntime` cache (multi-tenancy P2d): each tenant's own
    /// IdP config (`oidc_configs`) is built into a runtime lazily on first
    /// login and cached here, keyed by tenant id. Invalidated (best-effort) by
    /// `admin_oidc_config_put`/`_delete`; also self-expires via TTL.
    ///
    /// Moved out of the core `AppState` in LUC-145: with per-tenant IdP
    /// resolution behind `api::tenant_idp`, nothing in the core reads it. The
    /// cache type itself stays in `src/oidc.rs`, which is core: it is a TTL map
    /// over `OidcRuntime`, with no Enterprise logic of its own.
    pub oidc_tenants: crate::oidc::TenantOidcCache,
}

/// Enterprise boot, called once by `main`.
///
/// Collects what used to be scattered across the binary's startup: the
/// Keycloak admin runtime, the per-tenant provisioning backfill, and the
/// automatic subdomain seed. All three only make sense when operating quark as
/// a service, so they live here and not in the core (LUC-19).
///
/// Idempotent and cheap: it runs on every replica at every boot and skips what
/// is already done.
pub async fn boot(
    store: &std::sync::Arc<dyn crate::store::Store>,
    multi_tenant: bool,
    tenant_domain_suffix: Option<&str>,
) -> EeState {
    // Keycloak-hosted auth (multi-tenancy P2e, opt-in via
    // QUARK_KEYCLOAK_BASE_URL). Foundation only here: the trait + HTTP client +
    // config. The provisioning flow that calls it on tenant creation is a
    // later task.
    let keycloak_config = crate::ee::keycloak::KeycloakConfig::from_env();
    let keycloak_base_url = keycloak_config.as_ref().map(|c| c.base_url.clone());
    let keycloak: Option<Arc<dyn crate::ee::keycloak::KeycloakAdmin>> = match keycloak_config {
        Some(cfg) => {
            let base = cfg.base_url.clone();
            tracing::info!(base_url = %base, "keycloak admin enabled");
            Some(Arc::new(
                crate::ee::keycloak::client::HttpKeycloakAdmin::new(
                    cfg,
                    crate::ee::keycloak::client::keycloak_client(),
                ),
            ))
        }
        None => {
            tracing::info!("keycloak admin: disabled (set QUARK_KEYCLOAK_BASE_URL to enable)");
            None
        }
    };

    // Keycloak tenant provisioning boot backfill (multi-tenancy P2e Task 2):
    // every tenant that has no `oidc_config` yet (created before Keycloak was
    // configured, or whose creation-time attempt only got partway) gets
    // (re-)provisioned here. Idempotent and cheap, like the subdomain
    // backfill above — safe to run on every replica.
    if multi_tenant {
        if let (Some(kc), Some(base)) = (&keycloak, &keycloak_base_url) {
            match crate::ee::api::backfill_keycloak_provisioning(store, kc, base).await {
                Ok(n) => tracing::info!(provisioned = n, "keycloak tenant backfill completed"),
                Err(e) => tracing::warn!(
                    error = %e,
                    "keycloak tenant backfill skipped, could not list tenants"
                ),
            }
        }
    }

    // Auto per-tenant subdomain boot backfill (multi-tenancy P3-completion):
    // every existing tenant gets its `<slug>.<suffix>` `domains` row, same as
    // a freshly created one (`admin_tenants_create`). Idempotent (skips
    // tenants that already have the row) and cheap (few tenants, once per
    // boot) — safe to run on every replica.
    if multi_tenant {
        if let Some(suffix) = &tenant_domain_suffix {
            match store.list_tenants().await {
                Ok(tenants) => {
                    let mut seeded = 0usize;
                    for t in &tenants {
                        let host = crate::domain::subdomain_host(&t.slug, suffix);
                        match store.get_domain_by_host(&host).await {
                            Ok(Some(_)) => {} // already seeded
                            Ok(None) => {
                                match crate::ee::api::seed_tenant_subdomain(
                                    store, t.id, &t.slug, suffix,
                                )
                                .await
                                {
                                    Ok(()) => seeded += 1,
                                    Err(e) => tracing::warn!(
                                        error = %e,
                                        tenant_id = t.id.0,
                                        "tenant subdomain backfill failed"
                                    ),
                                }
                            }
                            Err(e) => tracing::warn!(
                                error = %e,
                                tenant_id = t.id.0,
                                "tenant subdomain backfill could not look up the domain row"
                            ),
                        }
                    }
                    tracing::info!(
                        seeded,
                        already_present = tenants.len() - seeded,
                        suffix = %suffix,
                        "tenant subdomain backfill completed"
                    );
                }
                Err(e) => tracing::warn!(
                    error = %e,
                    "tenant subdomain backfill skipped, could not list tenants"
                ),
            }
        }
    }

    EeState {
        keycloak,
        keycloak_base_url,
        oidc_tenants: crate::oidc::TenantOidcCache::new(),
    }
}
