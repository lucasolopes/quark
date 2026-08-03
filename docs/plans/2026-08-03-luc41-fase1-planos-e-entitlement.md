# Planos e entitlement (LUC-41, Fase 1) — plano de implementação

> **Para quem executa com agente:** SUB-SKILL OBRIGATÓRIA: use
> `superpowers:subagent-driven-development` (recomendado) ou
> `superpowers:executing-plans` para implementar tarefa a tarefa. Os passos usam
> checkbox (`- [ ]`) para acompanhamento.

**Objetivo:** fazer o quark cloud aplicar a grade de planos (limites e features
por tenant) sem integrar gateway de pagamento nenhum.

**Arquitetura:** o tenant guarda uma string com o nome do plano; o catálogo de
limites é `const` em `src/ee/plan.rs`. O core expõe um seam (`src/api/entitlement.rs`)
que na edição Community sempre permite, e a edição Enterprise implementa a
checagem real lendo o plano por um cache. Os gates são chamados só nos handlers
que têm gate, nunca no caminho de redirect.

**Stack:** Rust, axum 0.8, sqlx 0.9 (Postgres), heed 0.22 (LMDB), moka 0.12.

Spec: `docs/specs/2026-08-03-planos-e-entitlement-design.md`.
Grade: `docs/DECISAO-planos-e-pricing-cloud.md`.

## Restrições globais

Valem para toda tarefa, sem repetir em cada uma.

- `cargo fmt` e `cargo clippy --all-targets -- -D warnings` limpos nos **dois**
  modos: sem feature e com `--features ee`.
- **Apagar `src/ee/` tem que deixar o core compilando e passando.** É o job
  `community-only` do CI. Nenhum arquivo fora de `src/ee/` pode nomear um tipo
  que mora lá dentro.
- **A edição Community nunca aplica limite.** Todo caminho de gate devolve `Ok`
  sem consultar nada quando a feature `ee` está desligada.
- **Nada de entitlement no caminho de redirect.** Nem chamada, nem consulta.
- Comentário e doc comment de código em **inglês**; este plano e as specs em
  pt-BR (convenção do repo, `CLAUDE.md`).
- Testes gated de Postgres rodam com `QUARK_TEST_DATABASE_URL` apontando para
  role **não-superusuária** (`postgres://quark_test:quark_test@127.0.0.1:5432/quark_test`).
- Binária de teste que só exercita superfície EE começa com
  `#![cfg(feature = "ee")]`.
- `cargo` não está no PATH: use `~/.cargo/bin/cargo.exe`.

## Estrutura de arquivos

| Arquivo | Responsabilidade |
|---|---|
| `src/store/mod.rs` (modificar) | dois métodos novos no trait `Store`, plano como string opaca |
| `src/store/postgres.rs` (modificar) | migração da coluna e implementação real |
| `src/store/lmdb.rs` (modificar) | implementação inerte, como já faz para `primary_domain_id` |
| `src/api/entitlement.rs` (criar) | seam: `Feature`, `Quota`, `Denied`, e as versões Community |
| `src/ee/plan.rs` (criar) | catálogo: `Plan`, `Limits`, `PlanCache` |
| `src/ee/api/entitlement.rs` (criar) | implementação EE, mais os dois handlers de plano |
| `src/api/webhooks_api.rs`, `src/api/sheets.rs`, `src/api/links_admin.rs` (modificar) | gates do lado core |
| `src/ee/api/domains.rs`, `src/ee/api/invites.rs` (modificar) | gates do lado EE |
| `tests/plan_it.rs` (criar) | integração da grade, só EE |
| `tests/plan_community_it.rs` (criar) | prova que a Community não limita |

---

### Task 1: Store guarda o plano do tenant

O core armazena, não interpreta: o plano é uma `String` opaca para o `Store`.
Quem dá sentido a ela é `src/ee/plan.rs`. É isso que mantém a coluna no core
sem o core depender de tipo da EE.

**Files:**
- Modify: `src/store/mod.rs` (trait `Store`, perto de `get_primary_domain_id`, linha ~714)
- Modify: `src/store/postgres.rs` (migração no `init_schema` perto da linha 823; impl perto de `set_primary_domain`, linha ~2861)
- Modify: `src/store/lmdb.rs` (impl perto de `set_primary_domain`, linha ~1342)
- Test: `tests/postgres_store_it.rs`

**Interfaces:**
- Produces: `Store::get_tenant_plan(&self, tenant: TenantId) -> Result<Option<String>, StoreError>`,
  `Store::set_tenant_plan(&self, tenant: TenantId, plan: &str) -> Result<(), StoreError>` e
  `Store::count_memberships(&self, tenant: TenantId) -> Result<u64, StoreError>`.
  `None` no plano significa "sem plano gravado"; quem chama trata como o plano
  padrão.

**Por que `count_memberships` entra aqui.** O trait hoje só tem
`list_memberships_for_user`, que conta por usuário. Não existe caminho para
saber quantos membros um tenant tem, e sem isso a quota de membros da Task 6
não é aplicável. É um `COUNT` e cabe nesta task, que já abre os três arquivos de
store.

- [ ] **Step 1: escrever o teste que falha**

Em `tests/postgres_store_it.rs`, no fim do arquivo:

```rust
#[tokio::test]
#[file_serial]
async fn tenant_plan_round_trips_and_defaults_to_free_pg() {
    let Some(s) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let t = quark::tenant::TenantId(4242);
    s.put_tenant(&quark::tenant::Tenant {
        id: t,
        name: "Acme".into(),
        slug: "acme-plan".into(),
        created: 0,
    })
    .await
    .unwrap();

    // A fresh tenant carries the column default.
    assert_eq!(s.get_tenant_plan(t).await.unwrap().as_deref(), Some("free"));

    s.set_tenant_plan(t, "pro").await.unwrap();
    assert_eq!(s.get_tenant_plan(t).await.unwrap().as_deref(), Some("pro"));

    // An unknown tenant has no plan at all.
    assert_eq!(
        s.get_tenant_plan(quark::tenant::TenantId(999_999))
            .await
            .unwrap(),
        None
    );

    // A tenant with no memberships counts zero; the ceiling check relies on it.
    assert_eq!(s.count_memberships(t).await.unwrap(), 0);
}
```

Confira o nome do helper de setup no topo do arquivo (`fresh()` ou equivalente)
e use o que já existe ali, em vez de criar outro.

- [ ] **Step 2: rodar e ver falhar**

```bash
QUARK_TEST_DATABASE_URL='postgres://quark_test:quark_test@127.0.0.1:5432/quark_test' \
  ~/.cargo/bin/cargo.exe test --test postgres_store_it tenant_plan_round_trips
```

Esperado: FALHA de compilação, `no method named get_tenant_plan`.

- [ ] **Step 3: declarar no trait**

Em `src/store/mod.rs`, logo depois de `get_primary_domain_id`:

```rust
    /// The tenant's billing plan, as an opaque string. The core stores it and
    /// never interprets it: the catalog that gives it meaning is Enterprise
    /// (`src/ee/plan.rs`), which is why this is a `String` and not a typed enum.
    /// `None` means no row, or a backend that does not carry plans at all.
    async fn get_tenant_plan(&self, tenant: TenantId) -> Result<Option<String>, StoreError>;
    /// Sets the tenant's billing plan. Cloud-only, like `set_primary_domain`.
    async fn set_tenant_plan(&self, tenant: TenantId, plan: &str) -> Result<(), StoreError>;
    /// How many members the tenant has. The existing
    /// `list_memberships_for_user` answers the other direction and cannot be
    /// used for a per-tenant ceiling.
    async fn count_memberships(&self, tenant: TenantId) -> Result<u64, StoreError>;
```

- [ ] **Step 4: migração e implementação no Postgres**

Em `src/store/postgres.rs`, na lista de DDL do `init_schema`, logo abaixo do
`ALTER TABLE tenants ADD COLUMN IF NOT EXISTS primary_domain_id BIGINT`:

```rust
                // Billing plan per tenant (LUC-41 phase 1). Opaque here: the
                // catalog that interprets it lives in `src/ee/plan.rs`. Default
                // 'free' so a tenant created before billing existed reads as the
                // entry plan instead of NULL.
                "ALTER TABLE tenants ADD COLUMN IF NOT EXISTS plan TEXT NOT NULL DEFAULT 'free'",
```

E a implementação, ao lado de `set_primary_domain`:

```rust
    async fn get_tenant_plan(&self, tenant: TenantId) -> Result<Option<String>, StoreError> {
        let row = sqlx::query("SELECT plan FROM tenants WHERE id = $1")
            .bind(tenant.0 as i64)
            .fetch_optional(&self.read)
            .await
            .map_err(StoreError::backend)?;
        Ok(row.map(|r| r.get::<String, _>("plan")))
    }

    async fn set_tenant_plan(&self, tenant: TenantId, plan: &str) -> Result<(), StoreError> {
        // `tenants` is a global table (not RLS-scoped), so this mirrors
        // `set_primary_domain` and uses the bare pool.
        sqlx::query("UPDATE tenants SET plan = $2 WHERE id = $1")
            .bind(tenant.0 as i64)
            .bind(plan)
            .execute(&self.write)
            .await
            .map_err(StoreError::backend)?;
        Ok(())
    }

    async fn count_memberships(&self, tenant: TenantId) -> Result<u64, StoreError> {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM memberships WHERE tenant_id = $1")
            .bind(tenant.0 as i64)
            .fetch_one(&self.read)
            .await
            .map_err(StoreError::backend)?;
        Ok(n.max(0) as u64)
    }
```

Confirme o nome da coluna de tenant em `memberships` no DDL do `init_schema`
antes de rodar; se for diferente de `tenant_id`, ajuste a consulta.

- [ ] **Step 5: implementação no LMDB**

Em `src/store/lmdb.rs`, ao lado de `get_primary_domain_id`:

```rust
    // Billing plans are cloud-only, same reasoning as the primary domain above:
    // the OSS backend is single-tenant and never has a plan.
    async fn get_tenant_plan(&self, _tenant: TenantId) -> Result<Option<String>, StoreError> {
        Ok(None)
    }

    async fn set_tenant_plan(&self, _tenant: TenantId, _plan: &str) -> Result<(), StoreError> {
        Err(StoreError::Unsupported)
    }

    // `memberships` is keyed `user_id || tenant_id` (see `membership_key`),
    // so the tenant is the suffix and there is no range to prefix-scan: the
    // whole sub-db is walked and only the entries pointing at this tenant are
    // counted. OSS is usually single-tenant, but `put_membership` is a real
    // implementation (OIDC login writes one row per user against
    // `DEFAULT_TENANT`, see `src/oidc.rs`), so this counts for real instead
    // of assuming a fixed membership count.
    async fn count_memberships(&self, tenant: TenantId) -> Result<u64, StoreError> {
        let rtxn = self.env.read_txn()?;
        let suffix = tenant.0.to_be_bytes();
        let mut n: u64 = 0;
        for item in self.memberships.iter(&rtxn)? {
            let (key, _) = item?;
            if key.len() == 16 && key[8..16] == suffix {
                n += 1;
            }
        }
        Ok(n)
    }
```

- [ ] **Step 6: rodar e ver passar**

```bash
QUARK_TEST_DATABASE_URL='postgres://quark_test:quark_test@127.0.0.1:5432/quark_test' \
  ~/.cargo/bin/cargo.exe test --test postgres_store_it tenant_plan_round_trips
```

Esperado: PASSA.

- [ ] **Step 7: gate e commit**

```bash
~/.cargo/bin/cargo.exe fmt
~/.cargo/bin/cargo.exe clippy --all-targets -- -D warnings
~/.cargo/bin/cargo.exe clippy --all-targets --features ee -- -D warnings
git add src/store/mod.rs src/store/postgres.rs src/store/lmdb.rs tests/postgres_store_it.rs
git commit -m "feat(store): guardar o plano do tenant como string opaca (LUC-41)"
```

---

### Task 2: catálogo de planos na EE

**Files:**
- Create: `src/ee/plan.rs`
- Modify: `src/ee/mod.rs` (declarar `pub mod plan;`)

**Interfaces:**
- Consumes: nada.
- Produces: `Plan` (`Free`, `Starter`, `Pro`, `Business`, `Custom`), `Plan::ALL`,
  `Plan::from_stored(&str) -> Plan`, `Plan::as_str(self) -> &'static str`,
  `Plan::allows(self, Feature) -> bool`, `Plan::limits(self) -> Limits`,
  `Plan::cheapest_with(Feature) -> Option<Plan>`, e o struct `Limits`.
  `Feature` vem de `crate::api::entitlement`, criada na Task 3 — esta task
  compila só depois dela, então **execute a Task 3 antes desta** se estiver
  seguindo fora de ordem.

- [ ] **Step 1: escrever o teste que falha**

No fim de `src/ee/plan.rs` (o arquivo ainda não existe; crie com o teste
primeiro, o resto vem no Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::entitlement::Feature;

    /// Every feature a cheaper plan allows, a more expensive plan must also
    /// allow. A hole here means someone downgraded a tier by accident.
    #[test]
    fn feature_access_is_monotonic_up_the_ladder() {
        for f in Feature::ALL {
            let mut seen_allowed = false;
            for p in Plan::ALL {
                let allowed = p.allows(f);
                if seen_allowed {
                    assert!(allowed, "{p:?} denies {f:?} but a cheaper plan allows it");
                }
                seen_allowed |= allowed;
            }
        }
    }

    /// The numbers here are the contract in
    /// `docs/DECISAO-planos-e-pricing-cloud.md`. Changing one is a product
    /// decision, so it has to break this test on the way.
    #[test]
    fn limits_match_the_published_grid() {
        assert_eq!(Plan::Free.limits().domains, Some(3));
        assert_eq!(Plan::Free.limits().members, Some(1));
        assert_eq!(Plan::Starter.limits().domains, Some(10));
        assert_eq!(Plan::Starter.limits().members, Some(3));
        assert_eq!(Plan::Pro.limits().domains, Some(50));
        assert_eq!(Plan::Pro.limits().members, Some(10));
        assert_eq!(Plan::Business.limits().domains, None);
        assert_eq!(Plan::Custom.limits().members, None);
    }

    #[test]
    fn unknown_stored_value_falls_back_to_free() {
        assert_eq!(Plan::from_stored("free"), Plan::Free);
        assert_eq!(Plan::from_stored("pro"), Plan::Pro);
        assert_eq!(Plan::from_stored("nonsense"), Plan::Free);
    }

    #[test]
    fn cheapest_plan_with_a_feature_is_reported_for_the_upgrade_hint() {
        assert_eq!(Plan::cheapest_with(Feature::Webhooks), Some(Plan::Starter));
        assert_eq!(Plan::cheapest_with(Feature::Sso), Some(Plan::Business));
    }
}
```

- [ ] **Step 2: rodar e ver falhar**

```bash
~/.cargo/bin/cargo.exe test --features ee --lib ee::plan
```

Esperado: FALHA de compilação (`Plan` não existe).

- [ ] **Step 3: escrever o catálogo**

No topo de `src/ee/plan.rs`, antes do módulo de teste:

```rust
//! The plan catalog. Covered by `src/ee/LICENSE`, not by the AGPL.
//!
//! The numbers are the published grid in
//! `docs/DECISAO-planos-e-pricing-cloud.md`. They live in code, versioned
//! alongside the features they limit, so changing a limit is a deploy and
//! applies at once to everyone on that plan.

use crate::api::entitlement::Feature;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    Free,
    Starter,
    Pro,
    Business,
    Custom,
}

/// Numeric ceilings. `None` means unlimited.
///
/// Deliberately does NOT implement `Default`: adding a field must force every
/// plan below to state a value, instead of silently inheriting a zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub domains: Option<u32>,
    pub members: Option<u32>,
    pub automation_per_month: Option<u64>,
    pub tracked_clicks_per_month: Option<u64>,
    pub retention_days: Option<u32>,
}

impl Plan {
    /// Cheapest first. The order is what makes `cheapest_with` and the
    /// monotonicity test meaningful.
    pub const ALL: [Plan; 5] = [
        Plan::Free,
        Plan::Starter,
        Plan::Pro,
        Plan::Business,
        Plan::Custom,
    ];

    /// Parses the opaque string the store keeps. An unknown value falls back to
    /// `Free` rather than failing the request: a typo in the column must not
    /// hand out a better plan, and must not take the product down either.
    pub fn from_stored(s: &str) -> Plan {
        match s {
            "starter" => Plan::Starter,
            "pro" => Plan::Pro,
            "business" => Plan::Business,
            "custom" => Plan::Custom,
            _ => Plan::Free,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Plan::Free => "free",
            Plan::Starter => "starter",
            Plan::Pro => "pro",
            Plan::Business => "business",
            Plan::Custom => "custom",
        }
    }

    /// Whether this plan unlocks `f`.
    ///
    /// Nested exhaustive matches with NO wildcard arm, on purpose: adding a
    /// variant to `Feature` breaks the build here and lists every plan that
    /// still has to decide. A list of allowed features would instead fail
    /// silently, leaving the new feature denied everywhere.
    pub fn allows(self, f: Feature) -> bool {
        match self {
            Plan::Free => match f {
                Feature::Webhooks => false,
                Feature::Integrations => false,
                Feature::HealthMonitoring => false,
                Feature::TokenScopes => false,
                Feature::Sso => false,
            },
            Plan::Starter => match f {
                Feature::Webhooks => true,
                Feature::Integrations => true,
                Feature::HealthMonitoring => false,
                Feature::TokenScopes => false,
                Feature::Sso => false,
            },
            Plan::Pro => match f {
                Feature::Webhooks => true,
                Feature::Integrations => true,
                Feature::HealthMonitoring => true,
                Feature::TokenScopes => true,
                Feature::Sso => false,
            },
            Plan::Business => match f {
                Feature::Webhooks => true,
                Feature::Integrations => true,
                Feature::HealthMonitoring => true,
                Feature::TokenScopes => true,
                Feature::Sso => true,
            },
            Plan::Custom => match f {
                Feature::Webhooks => true,
                Feature::Integrations => true,
                Feature::HealthMonitoring => true,
                Feature::TokenScopes => true,
                Feature::Sso => true,
            },
        }
    }

    pub fn limits(self) -> Limits {
        match self {
            Plan::Free => Limits {
                domains: Some(3),
                members: Some(1),
                automation_per_month: Some(100),
                tracked_clicks_per_month: Some(50_000),
                retention_days: Some(30),
            },
            Plan::Starter => Limits {
                domains: Some(10),
                members: Some(3),
                automation_per_month: Some(5_000),
                tracked_clicks_per_month: Some(250_000),
                retention_days: Some(365),
            },
            Plan::Pro => Limits {
                domains: Some(50),
                members: Some(10),
                automation_per_month: Some(50_000),
                tracked_clicks_per_month: Some(1_000_000),
                retention_days: Some(730),
            },
            Plan::Business => Limits {
                domains: None,
                members: None,
                automation_per_month: Some(500_000),
                tracked_clicks_per_month: Some(5_000_000),
                retention_days: Some(1_095),
            },
            // Negotiated. The per-tenant override narrows this; absent an
            // override, Custom is unlimited.
            Plan::Custom => Limits {
                domains: None,
                members: None,
                automation_per_month: None,
                tracked_clicks_per_month: None,
                retention_days: None,
            },
        }
    }

    /// The cheapest plan that unlocks `f`, for the upgrade hint in a `402`.
    pub fn cheapest_with(f: Feature) -> Option<Plan> {
        Plan::ALL.into_iter().find(|p| p.allows(f))
    }
}
```

Em `src/ee/mod.rs`, junto dos outros `pub mod`:

```rust
pub mod plan;
```

- [ ] **Step 4: rodar e ver passar**

```bash
~/.cargo/bin/cargo.exe test --features ee --lib ee::plan
```

Esperado: PASSA, 4 testes.

- [ ] **Step 5: commit**

```bash
~/.cargo/bin/cargo.exe fmt
git add src/ee/plan.rs src/ee/mod.rs
git commit -m "feat(ee): catalogo de planos com match exaustivo (LUC-41)"
```

---

### Task 3: seam de entitlement no core

**Files:**
- Create: `src/api/entitlement.rs`
- Modify: `src/api/mod.rs` (declarar `pub(crate) mod entitlement;`)

**Interfaces:**
- Consumes: nada.
- Produces: `Feature` (`Webhooks`, `Integrations`, `HealthMonitoring`,
  `TokenScopes`, `Sso`) com `Feature::ALL`; `Quota` (`Domains`, `Members`) com
  `Quota::ALL`; `Denied { limit, allowed, upgrade_to }` implementando
  `IntoResponse` como `402`; e as funções
  `require(&AppState, TenantId, Feature) -> Result<(), Denied>` e
  `require_quota(&AppState, TenantId, Quota, u64) -> Result<(), Denied>`.

- [ ] **Step 1: escrever o teste que falha**

Crie `tests/plan_community_it.rs`:

```rust
// Codigo de teste pode entrar em panico: a falha e o proprio sinal.
#![allow(clippy::unwrap_used)]

//! The Community edition must never enforce a plan limit. A self-hosted AGPL
//! install is free and unlimited (LUC-19), so every gate resolves to `Ok`.
//!
//! This binary runs in BOTH builds on purpose: with `--features ee` the state
//! it builds has no plan configured, which must also resolve to the entry plan
//! without denying anything a Free tenant may do.

use quark::api::entitlement::{require, require_quota, Feature, Quota};
use quark::tenant::DEFAULT_TENANT;

mod common;

async fn state() -> std::sync::Arc<quark::api::AppState> {
    let dir = tempfile::tempdir().unwrap();
    let store = quark::store::open_store(dir.path()).await.unwrap();
    let sink = common::test_sink();
    common::TestState::new(store, sink).build()
}

#[tokio::test]
async fn community_allows_every_feature() {
    let st = state().await;
    for f in Feature::ALL {
        assert!(
            require(&st, DEFAULT_TENANT, f).await.is_ok(),
            "Community denied {f:?}"
        );
    }
}

#[tokio::test]
async fn community_allows_any_quota_usage() {
    let st = state().await;
    for q in Quota::ALL {
        assert!(
            require_quota(&st, DEFAULT_TENANT, q, 10_000).await.is_ok(),
            "Community denied {q:?} at 10k"
        );
    }
}
```

`TestState::new(store, sink)` é a assinatura real (`tests/common/mod.rs:73`).
Confirme o nome do helper de sink em `tests/common/mod.rs` e use o que existe
ali; se não houver, siga o que `tests/api_it.rs:20-32` faz para montar um.

- [ ] **Step 2: rodar e ver falhar**

```bash
~/.cargo/bin/cargo.exe test --test plan_community_it
```

Esperado: FALHA de compilação, `unresolved import quark::api::entitlement`.

- [ ] **Step 3: escrever o seam**

`src/api/entitlement.rs`:

```rust
//! Seam for plan enforcement (LUC-41 phase 1).
//!
//! Plans only exist when operating quark as a service for other people, so the
//! catalog and the real check live in `src/ee/`. What the core keeps is this
//! seam: the vocabulary (`Feature`, `Quota`, `Denied`) plus Community
//! implementations that allow everything.
//!
//! The Community edition MUST never enforce a limit. A self-hosted AGPL install
//! is free and unlimited; limiting it would contradict the open-core decision in
//! `docs/specs/2026-08-03-luc19-open-core-design.md`.
//!
//! Two `cfg`-selected functions rather than a trait object: the choice is made
//! at compile time and never varies at runtime.

use crate::tenant::TenantId;
use axum::response::{IntoResponse, Response};
use axum::http::StatusCode;
use axum::Json;

/// A capability a plan either unlocks or does not. Binary, no ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    Webhooks,
    Integrations,
    HealthMonitoring,
    TokenScopes,
    Sso,
}

impl Feature {
    pub const ALL: [Feature; 5] = [
        Feature::Webhooks,
        Feature::Integrations,
        Feature::HealthMonitoring,
        Feature::TokenScopes,
        Feature::Sso,
    ];

    /// Stable wire name, used in the `402` body and by the panel.
    pub fn as_str(self) -> &'static str {
        match self {
            Feature::Webhooks => "webhooks",
            Feature::Integrations => "integrations",
            Feature::HealthMonitoring => "health_monitoring",
            Feature::TokenScopes => "token_scopes",
            Feature::Sso => "sso",
        }
    }
}

/// A countable ceiling. Phase 1 covers only the ones answerable with a row
/// count; the monthly counters (automation, tracked clicks) arrive in phase 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quota {
    Domains,
    Members,
}

impl Quota {
    pub const ALL: [Quota; 2] = [Quota::Domains, Quota::Members];

    pub fn as_str(self) -> &'static str {
        match self {
            Quota::Domains => "domains",
            Quota::Members => "members",
        }
    }
}

/// Why the request was refused, and what fixes it.
///
/// Renders as `402 Payment Required`, not `403`: the caller does have
/// permission, what is missing is plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denied {
    /// `Feature::as_str` or `Quota::as_str`.
    pub limit: &'static str,
    /// The ceiling that was hit. `None` for a binary feature.
    pub allowed: Option<u64>,
    /// Cheapest plan that lifts it, so the panel can build the upgrade call
    /// without guessing.
    pub upgrade_to: &'static str,
}

impl IntoResponse for Denied {
    fn into_response(self) -> Response {
        (
            StatusCode::PAYMENT_REQUIRED,
            Json(serde_json::json!({
                "error": "plan_limit_reached",
                "limit": self.limit,
                "allowed": self.allowed,
                "upgrade_to": self.upgrade_to,
            })),
        )
            .into_response()
    }
}

/// Community: every feature is allowed.
#[cfg(not(feature = "ee"))]
pub async fn require(
    _st: &super::AppState,
    _tenant: TenantId,
    _f: Feature,
) -> Result<(), Denied> {
    Ok(())
}

/// Community: no ceiling applies.
#[cfg(not(feature = "ee"))]
pub async fn require_quota(
    _st: &super::AppState,
    _tenant: TenantId,
    _q: Quota,
    _current: u64,
) -> Result<(), Denied> {
    Ok(())
}

#[cfg(feature = "ee")]
pub use crate::ee::api::entitlement::{require, require_quota};
```

Em `src/api/mod.rs`, junto dos outros `mod`:

```rust
pub mod entitlement;
```

`pub`, e não `pub(crate)`, porque `tests/plan_community_it.rs` importa de fora
do crate.

- [ ] **Step 4: rodar e ver passar**

```bash
~/.cargo/bin/cargo.exe test --test plan_community_it
```

Esperado: PASSA, 2 testes.

- [ ] **Step 5: commit**

```bash
~/.cargo/bin/cargo.exe fmt
~/.cargo/bin/cargo.exe clippy --all-targets -- -D warnings
git add src/api/entitlement.rs src/api/mod.rs tests/plan_community_it.rs
git commit -m "feat(api): seam de entitlement, Community sempre permite (LUC-41)"
```

---

### Task 4: implementação EE do entitlement, com cache

**Files:**
- Create: `src/ee/api/entitlement.rs`
- Modify: `src/ee/api/mod.rs` (declarar o módulo)
- Modify: `src/ee/mod.rs` (campo de cache no `EeState`)
- Test: `tests/plan_it.rs`

**Interfaces:**
- Consumes: `Plan`, `Limits` (Task 2); `Feature`, `Quota`, `Denied` (Task 3);
  `Store::get_tenant_plan` (Task 1).
- Produces: `require`, `require_quota` com as mesmas assinaturas da Task 3, e
  `plan_of(&AppState, TenantId) -> Plan`, usada pelos handlers da Task 7.
  Mais `PlanCache::invalidate(TenantId)`.

- [ ] **Step 1: escrever o teste que falha**

Crie `tests/plan_it.rs`:

```rust
// Codigo de teste pode entrar em panico: a falha e o proprio sinal.
#![allow(clippy::unwrap_used)]

// Enterprise suite: plans only exist in the `--features ee` build (LUC-41).
#![cfg(feature = "ee")]

use quark::api::entitlement::{require, require_quota, Feature, Quota};
use quark::tenant::{Tenant, TenantId};

mod common;

async fn state_with_plan(plan: &str) -> (std::sync::Arc<quark::api::AppState>, TenantId) {
    let url = std::env::var("QUARK_TEST_DATABASE_URL").expect("gated test");
    let store = quark::store::postgres::PostgresStore::open(&url, true)
        .await
        .unwrap();
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
    let sink = common::test_sink();
    let st = common::TestState::new(std::sync::Arc::new(store), sink)
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
```

`TestState::new(store, sink)` é a assinatura real, e `multi_tenant(true)` é
necessário porque os handlers EE retornam 404 fora do modo cloud. Confirme o
nome do helper de sink em `tests/common/mod.rs`.

- [ ] **Step 2: rodar e ver falhar**

```bash
QUARK_TEST_DATABASE_URL='postgres://quark_test:quark_test@127.0.0.1:5432/quark_test' \
  ~/.cargo/bin/cargo.exe test --features ee --test plan_it
```

Esperado: FALHA de compilação.

- [ ] **Step 3: cache no `EeState`**

Em `src/ee/plan.rs`, no fim (antes do `mod tests`):

```rust
/// Per-tenant plan cache. The plan is read on every gated request, and the
/// store round-trip would otherwise be paid each time. Same shape the crate
/// already uses for `TenantOidcCache` and the host router: moka with a TTL,
/// plus explicit invalidation when the plan changes.
#[derive(Clone)]
pub struct PlanCache {
    cache: moka::future::Cache<crate::tenant::TenantId, Plan>,
}

/// Short enough that a plan change nobody invalidated still converges quickly,
/// long enough that the store is not hit per request.
const PLAN_TTL_SECS: u64 = 60;

impl PlanCache {
    pub fn new() -> PlanCache {
        PlanCache {
            cache: moka::future::Cache::builder()
                .time_to_live(std::time::Duration::from_secs(PLAN_TTL_SECS))
                .build(),
        }
    }

    pub async fn get(&self, tenant: crate::tenant::TenantId) -> Option<Plan> {
        self.cache.get(&tenant).await
    }

    pub async fn put(&self, tenant: crate::tenant::TenantId, plan: Plan) {
        self.cache.insert(tenant, plan).await;
    }

    pub async fn invalidate(&self, tenant: crate::tenant::TenantId) {
        self.cache.invalidate(&tenant).await;
    }
}

impl Default for PlanCache {
    fn default() -> Self {
        PlanCache::new()
    }
}
```

Em `src/ee/mod.rs`, no struct `EeState`, junto de `oidc_tenants`:

```rust
    /// Per-tenant plan cache (LUC-41 phase 1). See `plan::PlanCache`.
    pub plans: plan::PlanCache,
```

E no `EeState` construído por `boot`, adicione `plans: plan::PlanCache::new(),`.
Faça o mesmo no builder de `tests/common/mod.rs`, onde o `EeState` é montado.

- [ ] **Step 4: escrever a implementação**

`src/ee/api/entitlement.rs`:

```rust
//! Plan enforcement (LUC-41 phase 1). Covered by `src/ee/LICENSE`, not the AGPL.
//!
//! The Enterprise half of `api/entitlement.rs`: reads the tenant's plan and
//! answers against the catalog in `crate::ee::plan`.

use crate::api::entitlement::{Denied, Feature, Quota};
use crate::api::AppState;
use crate::ee::plan::Plan;
use crate::tenant::TenantId;

/// The tenant's plan, through the cache.
///
/// A store error resolves to `Free` rather than failing the request: a blip in
/// the plan lookup must not take a paying tenant's product down, and `Free` is
/// the safe direction (it can only deny, never hand out a better plan).
pub async fn plan_of(st: &AppState, tenant: TenantId) -> Plan {
    if let Some(p) = st.ee.plans.get(tenant).await {
        return p;
    }
    let p = match st.store.get_tenant_plan(tenant).await {
        Ok(Some(s)) => Plan::from_stored(&s),
        Ok(None) => Plan::Free,
        Err(_) => Plan::Free,
    };
    st.ee.plans.put(tenant, p).await;
    p
}

pub async fn require(st: &AppState, tenant: TenantId, f: Feature) -> Result<(), Denied> {
    if plan_of(st, tenant).await.allows(f) {
        return Ok(());
    }
    Err(Denied {
        limit: f.as_str(),
        allowed: None,
        upgrade_to: Plan::cheapest_with(f).map(Plan::as_str).unwrap_or("custom"),
    })
}

/// `current` is how many the tenant already holds. The call is made BEFORE
/// creating the next one, so the check is `current >= ceiling`.
pub async fn require_quota(
    st: &AppState,
    tenant: TenantId,
    q: Quota,
    current: u64,
) -> Result<(), Denied> {
    let plan = plan_of(st, tenant).await;
    let limits = plan.limits();
    let ceiling = match q {
        Quota::Domains => limits.domains,
        Quota::Members => limits.members,
    };
    let Some(ceiling) = ceiling else {
        return Ok(()); // unlimited
    };
    if current < u64::from(ceiling) {
        return Ok(());
    }
    Err(Denied {
        limit: q.as_str(),
        allowed: Some(u64::from(ceiling)),
        upgrade_to: cheapest_above(q, u64::from(ceiling)),
    })
}

/// Cheapest plan whose ceiling for `q` is above `ceiling` (or unlimited).
fn cheapest_above(q: Quota, ceiling: u64) -> &'static str {
    Plan::ALL
        .into_iter()
        .find(|p| {
            let l = p.limits();
            let c = match q {
                Quota::Domains => l.domains,
                Quota::Members => l.members,
            };
            c.is_none_or(|c| u64::from(c) > ceiling)
        })
        .map(Plan::as_str)
        .unwrap_or("custom")
}
```

Em `src/ee/api/mod.rs`, junto dos outros módulos:

```rust
pub(crate) mod entitlement;
```

- [ ] **Step 5: rodar e ver passar**

```bash
QUARK_TEST_DATABASE_URL='postgres://quark_test:quark_test@127.0.0.1:5432/quark_test' \
  ~/.cargo/bin/cargo.exe test --features ee --test plan_it
```

Esperado: PASSA, 4 testes.

- [ ] **Step 6: gate e commit**

```bash
~/.cargo/bin/cargo.exe fmt
~/.cargo/bin/cargo.exe clippy --all-targets -- -D warnings
~/.cargo/bin/cargo.exe clippy --all-targets --features ee -- -D warnings
git add src/ee/ tests/plan_it.rs tests/common/mod.rs
git commit -m "feat(ee): checagem de entitlement com cache por tenant (LUC-41)"
```

---

### Task 5: aplicar os gates de feature no core

**Files:**
- Modify: `src/api/webhooks_api.rs` (`admin_webhooks_create`, linha ~227)
- Modify: `src/api/sheets.rs` (`sheets_connect`)
- Modify: `src/api/links_admin.rs` (criação de pixel, `admin_pixels_create`)
- Test: `tests/plan_it.rs`

**Interfaces:**
- Consumes: `require`, `Feature`, `Denied` (Tasks 3 e 4).
- Produces: nada novo.

- [ ] **Step 1: escrever o teste que falha**

Acrescente a `tests/plan_it.rs`:

```rust
#[tokio::test]
#[serial_test::file_serial]
async fn free_tenant_gets_402_creating_a_webhook() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    let (st, t) = state_with_plan("free").await;
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
    let _ = t;
}
```

Declare `const ADMIN_TOKEN: &str = "test-admin-token";` no topo do arquivo e
passe `.admin_token(Some(ADMIN_TOKEN.into()))` no builder do `state_with_plan`.
Use `tower::ServiceExt` para o `oneshot`, como as outras binárias fazem.

**Atenção ao tenant.** O break-glass resolve para `DEFAULT_TENANT` (0), não para
o tenant `t` criado no setup. Ou grave o plano em `DEFAULT_TENANT`, ou
autentique com um token de API pertencente a `t`. Um teste que grava o plano em
`t` e autentica pelo break-glass passa por engano, porque estaria medindo o
plano do tenant 0.

- [ ] **Step 2: rodar e ver falhar**

```bash
QUARK_TEST_DATABASE_URL='postgres://quark_test:quark_test@127.0.0.1:5432/quark_test' \
  ~/.cargo/bin/cargo.exe test --features ee --test plan_it free_tenant_gets_402
```

Esperado: FALHA, recebe `201` em vez de `402`.

- [ ] **Step 3: aplicar o gate**

Em `src/api/webhooks_api.rs`, dentro de `admin_webhooks_create`, logo depois do
`admin_guard` e antes de qualquer validação de URL:

```rust
    if let Err(denied) =
        crate::api::entitlement::require(&st, p.tenant, crate::api::entitlement::Feature::Webhooks)
            .await
    {
        return denied.into_response();
    }
```

Em `src/api/sheets.rs`, no início de `sheets_connect`, depois do guard, o mesmo
com `Feature::Integrations`.

Em `src/api/links_admin.rs`, no início de `admin_pixels_create`, depois do
guard, o mesmo com `Feature::Integrations`.

- [ ] **Step 4: rodar e ver passar**

```bash
QUARK_TEST_DATABASE_URL='postgres://quark_test:quark_test@127.0.0.1:5432/quark_test' \
  ~/.cargo/bin/cargo.exe test --features ee --test plan_it
```

Esperado: PASSA.

- [ ] **Step 5: provar que a Community não regrediu**

```bash
QUARK_TEST_DATABASE_URL='postgres://quark_test:quark_test@127.0.0.1:5432/quark_test' \
  ~/.cargo/bin/cargo.exe test --no-fail-fast
```

Esperado: zero falhas. Os testes de webhook, Sheets e pixel que já existiam
continuam passando, porque sem a feature `ee` o gate é `Ok`.

- [ ] **Step 6: commit**

```bash
~/.cargo/bin/cargo.exe fmt
~/.cargo/bin/cargo.exe clippy --all-targets --features ee -- -D warnings
git add src/api/webhooks_api.rs src/api/sheets.rs src/api/links_admin.rs tests/plan_it.rs
git commit -m "feat(api): gate de webhooks e integracoes por plano (LUC-41)"
```

---

### Task 6: aplicar os gates de quota na EE

**Files:**
- Modify: `src/ee/api/domains.rs` (`admin_domains_create`)
- Modify: `src/ee/api/invites.rs` (`admin_invites_create` e `admin_oidc_config_put`)
- Test: `tests/plan_it.rs`

**Interfaces:**
- Consumes: `require`, `require_quota`, `Feature`, `Quota` (Tasks 3 e 4).
- Produces: nada novo.

- [ ] **Step 1: escrever o teste que falha**

Acrescente a `tests/plan_it.rs`:

```rust
#[tokio::test]
#[serial_test::file_serial]
async fn free_tenant_gets_402_on_the_fourth_domain() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    let (st, t) = state_with_plan("free").await;
    // Seed three domains directly through the store: this test is about the
    // ceiling, not about the create endpoint's own validation.
    for i in 0..3u64 {
        let id = st.store.next_domain_id().await.unwrap();
        st.store
            .put_domain(&quark::domain::Domain {
                id,
                tenant_id: t,
                host: format!("d{i}.example.com"),
                token: String::new(),
                status: quark::domain::DomainStatus::Verified,
                created: 0,
                verified_at: None,
            })
            .await
            .unwrap();
    }
    let denied = quark::api::entitlement::require_quota(
        &st,
        t,
        quark::api::entitlement::Quota::Domains,
        3,
    )
    .await
    .unwrap_err();
    assert_eq!(denied.allowed, Some(3));
}
```

Confira os campos exatos de `Domain` em `src/domain.rs` antes de escrever o
literal; o struct acima segue o que está lá hoje.

- [ ] **Step 2: rodar e ver falhar**

```bash
QUARK_TEST_DATABASE_URL='postgres://quark_test:quark_test@127.0.0.1:5432/quark_test' \
  ~/.cargo/bin/cargo.exe test --features ee --test plan_it fourth_domain
```

Esperado: FALHA enquanto o gate não estiver no handler (o teste acima já passa
pela função; o passo 3 é o que liga no endpoint).

- [ ] **Step 3: aplicar os gates**

Em `src/ee/api/domains.rs`, dentro de `admin_domains_create`, depois do
`admin_guard` e da guarda de `multi_tenant`, antes de criar:

```rust
    let held = match st.store.list_domains(p.tenant).await {
        Ok(d) => d.len() as u64,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    if let Err(denied) = crate::api::entitlement::require_quota(
        &st,
        p.tenant,
        crate::api::entitlement::Quota::Domains,
        held,
    )
    .await
    {
        return denied.into_response();
    }
```

Em `src/ee/api/invites.rs`, dentro de `admin_invites_create`, o mesmo padrão
com `Quota::Members`, usando o `count_memberships` criado na Task 1:

```rust
    let held = match st.store.count_memberships(p.tenant).await {
        Ok(n) => n,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    if let Err(denied) = crate::api::entitlement::require_quota(
        &st,
        p.tenant,
        crate::api::entitlement::Quota::Members,
        held,
    )
    .await
    {
        return denied.into_response();
    }
```

O convite pendente NÃO conta para o teto: quem ainda não aceitou não ocupa
assento, e contar convite faria um convite recusado bloquear a vaga para
sempre.

Ainda em `src/ee/api/invites.rs`, no início de `admin_oidc_config_put`, depois
do guard:

```rust
    if let Err(denied) =
        crate::api::entitlement::require(&st, p.tenant, crate::api::entitlement::Feature::Sso)
            .await
    {
        return denied.into_response();
    }
```

- [ ] **Step 4: rodar e ver passar**

```bash
QUARK_TEST_DATABASE_URL='postgres://quark_test:quark_test@127.0.0.1:5432/quark_test' \
  ~/.cargo/bin/cargo.exe test --features ee --no-fail-fast
```

Esperado: zero falhas. Atenção: os testes de `domains_it`, `invites_it` e
`oidc_config_it` que já existem passam a rodar contra um tenant sem plano
gravado, que resolve para `Free`. Se algum deles criar mais de 3 domínios ou
convidar mais de 1 membro, ele vai passar a receber `402` — nesse caso o teste
deve gravar um plano compatível no setup, e não o gate ser afrouxado.

- [ ] **Step 5: commit**

```bash
~/.cargo/bin/cargo.exe fmt
~/.cargo/bin/cargo.exe clippy --all-targets --features ee -- -D warnings
git add src/ee/api/domains.rs src/ee/api/invites.rs tests/plan_it.rs
git commit -m "feat(ee): teto de dominio e membro, e SSO por plano (LUC-41)"
```

---

### Task 7: expor o plano e permitir trocá-lo

Sem gateway, a troca de plano é operação do operador do cloud. O endpoint de
escrita exige o break-glass `QUARK_ADMIN_TOKEN`, e não um token de API de
tenant: nenhum cliente pode promover o próprio plano.

**Files:**
- Modify: `src/ee/api/entitlement.rs` (dois handlers)
- Modify: `src/ee/api/mod.rs` (montar as rotas)
- Create: `docs/PLANS.md` e `docs/PLANS.PT_BR.md`
- Test: `tests/plan_it.rs`

**Interfaces:**
- Consumes: `plan_of` (Task 4), `Plan` (Task 2).
- Produces: `GET /admin/plan` e `PUT /admin/tenants/{id}/plan`.

- [ ] **Step 1: escrever o teste que falha**

Acrescente a `tests/plan_it.rs`:

```rust
#[tokio::test]
#[serial_test::file_serial]
async fn plan_endpoint_reports_the_grid_for_the_panel() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    let (st, _t) = state_with_plan("starter").await;
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
    let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(v["plan"].is_string());
    assert!(v["limits"]["domains"].is_number());
    assert!(v["features"].is_array());
}
```

Mesma atenção da Task 5: o break-glass resolve para `DEFAULT_TENANT`, então
grave o plano nesse tenant ou autentique com token de API de `t`.

- [ ] **Step 2: rodar e ver falhar**

```bash
QUARK_TEST_DATABASE_URL='postgres://quark_test:quark_test@127.0.0.1:5432/quark_test' \
  ~/.cargo/bin/cargo.exe test --features ee --test plan_it plan_endpoint
```

Esperado: FALHA com `404`.

- [ ] **Step 3: escrever os handlers**

No fim de `src/ee/api/entitlement.rs`:

```rust
use crate::api::{admin_guard, constant_time_eq};
use crate::auth::Scope;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use std::sync::Arc;

/// `GET /admin/plan`: the tenant's plan, its ceilings and its unlocked
/// features.
///
/// The panel renders from this instead of carrying its own copy of the grid,
/// which would drift on the first change.
pub(crate) async fn admin_plan_get(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let p = match admin_guard(&st, &headers, Scope::LinksRead).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    let plan = plan_of(&st, p.tenant).await;
    let l = plan.limits();
    let features: Vec<&'static str> = Feature::ALL
        .into_iter()
        .filter(|f| plan.allows(*f))
        .map(Feature::as_str)
        .collect();
    Json(serde_json::json!({
        "plan": plan.as_str(),
        "limits": {
            "domains": l.domains,
            "members": l.members,
            "automation_per_month": l.automation_per_month,
            "tracked_clicks_per_month": l.tracked_clicks_per_month,
            "retention_days": l.retention_days,
        },
        "features": features,
    }))
    .into_response()
}

#[derive(serde::Deserialize)]
pub(crate) struct SetPlanReq {
    pub plan: String,
}

/// `PUT /admin/tenants/{id}/plan`: operator-only plan change.
///
/// Requires the break-glass `QUARK_ADMIN_TOKEN` directly, not a tenant API
/// token: a customer must never be able to promote their own plan. Phase 2
/// replaces the manual call with the Stripe webhook, and this endpoint stays as
/// the operator escape hatch.
pub(crate) async fn admin_tenant_plan_put(
    State(st): State<Arc<AppState>>,
    Path(id): Path<u64>,
    headers: HeaderMap,
    Json(req): Json<SetPlanReq>,
) -> Response {
    let provided = headers
        .get(crate::api::HEADER_ADMIN_TOKEN)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let ok = st
        .admin_token
        .as_deref()
        .is_some_and(|expected| constant_time_eq(provided.as_bytes(), expected.as_bytes()));
    if !ok {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let plan = Plan::from_stored(&req.plan);
    // Reject a typo instead of silently downgrading the tenant to Free.
    if plan.as_str() != req.plan {
        return (StatusCode::BAD_REQUEST, "unknown plan").into_response();
    }
    let tenant = TenantId(id);
    if let Err(_e) = st.store.set_tenant_plan(tenant, plan.as_str()).await {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    st.ee.plans.invalidate(tenant).await;
    StatusCode::NO_CONTENT.into_response()
}
```

Confirme que `constant_time_eq` e `HEADER_ADMIN_TOKEN` estão acessíveis a partir
de `crate::api` (os dois são `pub(crate)` em `src/api/router.rs` e
`src/api/mod.rs`).

Em `src/ee/api/mod.rs`, dentro de `mount`, some as duas rotas:

```rust
        .route("/admin/plan", get(admin_plan_get))
        .route(
            "/admin/tenants/{id}/plan",
            axum::routing::put(admin_tenant_plan_put),
        )
```

- [ ] **Step 4: rodar e ver passar**

```bash
QUARK_TEST_DATABASE_URL='postgres://quark_test:quark_test@127.0.0.1:5432/quark_test' \
  ~/.cargo/bin/cargo.exe test --features ee --test plan_it
```

Esperado: PASSA.

- [ ] **Step 5: documentar**

Crie `docs/PLANS.md` com o cabeçalho de troca de idioma
(`**English** · [Português](PLANS.PT_BR.md)`) e o espelho em
`docs/PLANS.PT_BR.md`. Conteúdo: a grade de limites, quais features cada plano
libera, o comportamento de `402`, o fato de a edição Community não aplicar
limite nenhum, e como o operador troca o plano de um tenant. Diga
explicitamente que a página de preços de marketing é cópia, e não fonte.

Some uma linha em `docs/LICENSING.md` e no twin, na seção de variáveis
Enterprise, apontando `docs/PLANS.md`.

- [ ] **Step 6: gate completo e commit**

```bash
~/.cargo/bin/cargo.exe fmt
~/.cargo/bin/cargo.exe clippy --all-targets -- -D warnings
~/.cargo/bin/cargo.exe clippy --all-targets --features ee -- -D warnings
QUARK_TEST_DATABASE_URL='postgres://quark_test:quark_test@127.0.0.1:5432/quark_test' \
  ~/.cargo/bin/cargo.exe test --no-fail-fast
QUARK_TEST_DATABASE_URL='postgres://quark_test:quark_test@127.0.0.1:5432/quark_test' \
  ~/.cargo/bin/cargo.exe test --features ee --no-fail-fast
git add -A
git commit -m "feat(ee): endpoint de plano e troca pelo operador, mais docs (LUC-41)"
```

- [ ] **Step 7: provar a separação open-core**

```bash
rm -rf src/ee web/src/ee
~/.cargo/bin/cargo.exe build
~/.cargo/bin/cargo.exe test --no-fail-fast
git checkout -- src/ee web/src/ee
```

Esperado: compila e passa sem as pastas. É o mesmo que o job `community-only`
faz no CI; rodar local antes evita descobrir no PR.

- [ ] **Step 8: provar que o redirect não chama entitlement**

```bash
grep -n "entitlement" src/api/links.rs src/domain_router.rs src/cache/mod.rs
```

Esperado: **nenhuma linha**. A spec proíbe qualquer checagem de plano no caminho
de redirect, e isso é verificado por ausência. Se aparecer alguma, é regressão e
tem que sair.

---

## Fora deste plano

Stripe inteiro (Fase 2). Soft cap de analytics, contador mensal de cliques e
teto de automação por API (Fase 3, os três compartilham a máquina de contagem
mensal). Tela de billing no painel, que depende da Fase 2. Trial. Override de
limites por tenant no tier Custom, que só ganha uso quando houver cliente
negociado; a coluna `plan_limits` da spec fica de fora desta fase por não ter
consumidor ainda.
