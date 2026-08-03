//! quark Enterprise Edition.
//!
//! ATENCAO: este diretorio NAO e AGPL. Ele e coberto pela quark Enterprise
//! Edition License em `src/ee/LICENSE`. Todo o resto do repositorio e
//! AGPL-3.0-only. Ver `docs/LICENSING.md`.
//!
//! O modulo inteiro vive atras da cargo feature `ee`, que nao e default:
//!
//! ```text
//! cargo build                 # edicao Community, sem nada daqui
//! cargo build --features ee   # edicao Enterprise
//! ```
//!
//! Apagar `src/ee/` tem que deixar o core compilando e passando nos testes.
//! O CI prova isso a cada push; se essa propriedade quebrar, a separacao virou
//! decoracao. A regra do corte esta em
//! `docs/specs/2026-08-03-luc19-open-core-design.md`: fica no core o que uma
//! organizacao usa para si mesma, vem para ca o que so serve para operar o
//! quark como servico para terceiros.

pub mod api;
pub mod keycloak;

use std::sync::Arc;

/// Os campos do `AppState` que so existem na edicao Enterprise.
///
/// Deliberadamente pequeno. So entra aqui o campo que nomeia um tipo que sai do
/// core junto com esta pasta: sem isso, `AppState` nao compilaria com `src/ee/`
/// ausente. Campos que o core le continuam no core, mesmo sendo usados so em
/// cloud (`tenant_domain_suffix` e lido por `api/links.rs`, `oidc_tenants` e
/// cache sobre o store, `host_router` esta no hot path).
#[derive(Clone, Default)]
pub struct EeState {
    /// Runtime admin do Keycloak, presente so quando `QUARK_KEYCLOAK_BASE_URL`
    /// esta configurado. `None` desliga o provisionamento de realm por tenant.
    pub keycloak: Option<Arc<dyn keycloak::KeycloakAdmin>>,
    /// URL base em que o Keycloak responde, guardada junto para derivar o
    /// issuer de um tenant sem reler o ambiente.
    pub keycloak_base_url: Option<String>,
}

/// Boot da edicao Enterprise, chamado uma vez por `main`.
///
/// Junta o que antes estava espalhado pelo boot do binario: runtime admin do
/// Keycloak, backfill de provisionamento por tenant e seed do subdominio
/// automatico. Os tres so fazem sentido operando o quark como servico, entao
/// moram aqui e nao no core (LUC-19).
///
/// Idempotente e barato: roda em toda replica, a cada boot, e pula o que ja
/// esta feito.
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
    }
}
