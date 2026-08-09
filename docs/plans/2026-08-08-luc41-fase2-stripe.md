# Stripe (LUC-41, Fase 2) — plano de implementação

> **Para quem executa com agente:** SUB-SKILL OBRIGATÓRIA: use
> `superpowers:subagent-driven-development` (recomendado) ou
> `superpowers:executing-plans` para implementar tarefa a tarefa. Os passos usam
> checkbox (`- [ ]`) para acompanhamento.

**Objetivo:** ligar a cobrança do quark cloud: customer Stripe por tenant,
checkout hospedado, Customer Portal e webhook idempotente que escreve o plano.

**Arquitetura:** o SDK é o async-stripe 1.0.0-rc pinado (crates split por
área). A config vive em `src/ee/stripe/` como `Option` no `EeState` (padrão
Keycloak: sem env, billing não existe e os endpoints respondem 404). O plano
do tenant só é escrito pelo handler de webhook (e pelo escape hatch do
operador da Fase 1); dedup por event id em tabela própria; estado de
assinatura sempre buscado na API, nunca do payload.

**Stack:** Rust, axum 0.8, sqlx 0.9 (Postgres), heed 0.22 (LMDB),
async-stripe 1.0.0-rc.8.

Spec: `docs/specs/2026-08-08-luc41-fase2-stripe-design.md`.
API do SDK: `docs/research/2026-08-08-async-stripe-rc-api.md`.
Pesquisa geral: `docs/research/2026-08-08-luc41-fase2-stripe-research.md`.

## Restrições globais

Valem para toda tarefa, sem repetir em cada uma.

- `cargo fmt` e `cargo clippy --all-targets -- -D warnings` limpos nos **dois**
  modos: sem feature e com `--features ee`.
- **Apagar `src/ee/` tem que deixar o core compilando e passando** (job
  `community-only` do CI). Os crates do Stripe são `optional = true` e só
  entram pela feature `ee`; nenhum arquivo fora de `src/ee/` pode nomear um
  tipo `stripe_*`.
- **Nada de billing no caminho de redirect.** Nem chamada, nem consulta.
- O plano do tenant só é escrito por: handler de webhook e
  `PUT /admin/tenants/{id}/plan` (Fase 1). Nenhum endpoint novo escreve
  `plan` diretamente.
- Versões do async-stripe pinadas com `=1.0.0-rc.8`; sobem só por decisão.
- Comentário e doc comment de código em **inglês**; plano e specs em pt-BR.
- Testes gated de Postgres rodam com `QUARK_TEST_DATABASE_URL` apontando para
  role **não-superusuária**
  (`postgres://quark_test:quark_test@127.0.0.1:5432/quark_test`).
- Binária de teste que só exercita superfície EE começa com
  `#![cfg(feature = "ee")]`.
- `cargo` não está no PATH: use `~/.cargo/bin/cargo.exe`.

## Estrutura de arquivos

| Arquivo | Responsabilidade |
|---|---|
| `Cargo.toml` (modificar) | crates async-stripe pinados, opcionais, na feature `ee` |
| `src/store/mod.rs` (modificar) | 6 métodos novos no trait `Store`, tudo string opaca |
| `src/store/postgres.rs` (modificar) | DDL (2 colunas + tabela `stripe_events`) e implementação |
| `src/store/lmdb.rs` (modificar) | implementação inerte, como o plano da Fase 1 |
| `src/ee/stripe/mod.rs` (criar) | `StripeBilling`: config de env + client |
| `src/ee/stripe/map.rs` (criar) | lookup keys e mapeamento status→plano, puro |
| `src/ee/api/billing.rs` (criar) | `require_owner`, checkout, portal, webhook |
| `src/ee/api/mod.rs` (modificar) | montar as 3 rotas |
| `src/ee/mod.rs` (modificar) | `EeState.billing` + boot |
| `tests/common/mod.rs` (modificar) | campo `billing` no builder |
| `tests/billing_it.rs` (criar) | integração, só EE, Postgres gated |
| `docs/BILLING.md` + `.PT_BR.md` (criar) | doc de usuário |
| `docs/RUNBOOK-stripe.md` (criar) | setup único do dashboard |

---

### Task 1: Store guarda os vínculos do Stripe

O core armazena, não interpreta: customer id, subscription id e event id são
strings opacas, como `plan` na Fase 1. O lookup reverso existe porque o
webhook chega com customer id, não tenant id.

**Files:**
- Modify: `src/store/mod.rs` (trait, logo depois de `set_tenant_plan`, linha ~721)
- Modify: `src/store/postgres.rs` (DDL no `init_schema`; impl perto de `set_tenant_plan`, linha ~2908)
- Modify: `src/store/lmdb.rs` (impl perto de `set_tenant_plan`, linha ~1360)
- Modify: os mocks de `Store` em testes. Encontre todos com
  `grep -rn "async fn get_tenant_plan" src tests` (hoje: `src/domain_router.rs`
  ~618 e `src/webhooks/delivery.rs` ~1677) e implemente os 7 métodos novos
  neles com os mesmos corpos inertes do LMDB.
- Test: `tests/postgres_store_it.rs`

**Interfaces:**
- Produces:
  `Store::get_stripe_customer_id(&self, tenant: TenantId) -> Result<Option<String>, StoreError>`,
  `Store::set_stripe_customer_id(&self, tenant: TenantId, customer_id: &str) -> Result<(), StoreError>`,
  `Store::get_stripe_subscription_id(&self, tenant: TenantId) -> Result<Option<String>, StoreError>`,
  `Store::set_stripe_subscription_id(&self, tenant: TenantId, subscription_id: &str) -> Result<(), StoreError>`,
  `Store::find_tenant_by_stripe_customer(&self, customer_id: &str) -> Result<Option<TenantId>, StoreError>`,
  `Store::record_stripe_event(&self, id: &str, event_type: &str, received_at: u64) -> Result<bool, StoreError>`
  (`Ok(true)` gravou, `Ok(false)` já existia) e
  `Store::delete_stripe_event(&self, id: &str) -> Result<(), StoreError>`
  (solta o ledger quando o processamento falhou, para o retry do Stripe não
  cair no dedup).

- [ ] **Step 1: escrever o teste que falha**

Em `tests/postgres_store_it.rs`, no fim do arquivo (use o helper de setup que
o arquivo já tem, `fresh()` ou equivalente):

```rust
#[tokio::test]
#[file_serial]
async fn stripe_billing_columns_round_trip_pg() {
    let Some(s) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let t = quark::tenant::TenantId(4243);
    s.put_tenant(&quark::tenant::Tenant {
        id: t,
        name: "Acme".into(),
        slug: "acme-stripe".into(),
        created: 0,
    })
    .await
    .unwrap();

    // No billing yet.
    assert_eq!(s.get_stripe_customer_id(t).await.unwrap(), None);
    assert_eq!(s.get_stripe_subscription_id(t).await.unwrap(), None);
    assert_eq!(
        s.find_tenant_by_stripe_customer("cus_none").await.unwrap(),
        None
    );

    s.set_stripe_customer_id(t, "cus_123").await.unwrap();
    assert_eq!(
        s.get_stripe_customer_id(t).await.unwrap().as_deref(),
        Some("cus_123")
    );
    assert_eq!(
        s.find_tenant_by_stripe_customer("cus_123").await.unwrap(),
        Some(t)
    );

    s.set_stripe_subscription_id(t, "sub_123").await.unwrap();
    assert_eq!(
        s.get_stripe_subscription_id(t).await.unwrap().as_deref(),
        Some("sub_123")
    );

    // Event dedup: first insert true, replay false.
    assert!(s.record_stripe_event("evt_1", "invoice.paid", 100).await.unwrap());
    assert!(!s.record_stripe_event("evt_1", "invoice.paid", 101).await.unwrap());

    // Deleting frees the id again: this is what lets a Stripe retry through
    // after our own processing failed.
    s.delete_stripe_event("evt_1").await.unwrap();
    assert!(s.record_stripe_event("evt_1", "invoice.paid", 102).await.unwrap());
}
```

- [ ] **Step 2: rodar e ver falhar**

```bash
QUARK_TEST_DATABASE_URL='postgres://quark_test:quark_test@127.0.0.1:5432/quark_test' \
  ~/.cargo/bin/cargo.exe test --test postgres_store_it stripe_billing_columns
```

Esperado: FALHA de compilação, `no method named get_stripe_customer_id`.

- [ ] **Step 3: declarar no trait**

Em `src/store/mod.rs`, logo depois de `set_tenant_plan`:

```rust
    /// The tenant's Stripe customer id, opaque to the core (LUC-41 phase 2).
    /// `None` means the tenant never started a checkout, or a backend that
    /// carries no billing at all.
    async fn get_stripe_customer_id(&self, tenant: TenantId)
        -> Result<Option<String>, StoreError>;
    /// Persists the Stripe customer id, written once at first checkout.
    async fn set_stripe_customer_id(&self, tenant: TenantId, customer_id: &str)
        -> Result<(), StoreError>;
    /// The tenant's latest Stripe subscription id. Kept after cancellation on
    /// purpose: its presence is what marks "already had a trial".
    async fn get_stripe_subscription_id(&self, tenant: TenantId)
        -> Result<Option<String>, StoreError>;
    async fn set_stripe_subscription_id(&self, tenant: TenantId, subscription_id: &str)
        -> Result<(), StoreError>;
    /// Reverse lookup for the webhook, which arrives with a customer id.
    async fn find_tenant_by_stripe_customer(&self, customer_id: &str)
        -> Result<Option<TenantId>, StoreError>;
    /// Idempotency ledger for Stripe webhook events. `Ok(true)` recorded,
    /// `Ok(false)` the id was already there and the event must be skipped.
    async fn record_stripe_event(&self, id: &str, event_type: &str, received_at: u64)
        -> Result<bool, StoreError>;
    /// Removes an event from the ledger. Called when OUR processing failed
    /// after recording, so Stripe's retry of the same id is not swallowed by
    /// the dedup.
    async fn delete_stripe_event(&self, id: &str) -> Result<(), StoreError>;
```

- [ ] **Step 4: DDL e implementação no Postgres**

Em `src/store/postgres.rs`, na lista de DDL do `init_schema`, logo abaixo do
`ALTER TABLE tenants ... plan`:

```rust
                // Stripe billing (LUC-41 phase 2). Opaque ids; the meaning
                // lives in `src/ee/stripe/`.
                "ALTER TABLE tenants ADD COLUMN IF NOT EXISTS stripe_customer_id TEXT",
                "ALTER TABLE tenants ADD COLUMN IF NOT EXISTS stripe_subscription_id TEXT",
                // Webhook idempotency ledger: one row per delivered event id.
                "CREATE TABLE IF NOT EXISTS stripe_events (
                    id TEXT PRIMARY KEY,
                    type TEXT NOT NULL,
                    received_at BIGINT NOT NULL
                )",
```

Implementação, ao lado de `set_tenant_plan` (mesmo padrão: `tenants` é tabela
global, sem RLS, pools `read`/`write` diretos):

```rust
    async fn get_stripe_customer_id(
        &self,
        tenant: TenantId,
    ) -> Result<Option<String>, StoreError> {
        let row = sqlx::query("SELECT stripe_customer_id FROM tenants WHERE id = $1")
            .bind(tenant.0 as i64)
            .fetch_optional(&self.read)
            .await
            .map_err(StoreError::backend)?;
        Ok(row.and_then(|r| r.get::<Option<String>, _>("stripe_customer_id")))
    }

    async fn set_stripe_customer_id(
        &self,
        tenant: TenantId,
        customer_id: &str,
    ) -> Result<(), StoreError> {
        sqlx::query("UPDATE tenants SET stripe_customer_id = $2 WHERE id = $1")
            .bind(tenant.0 as i64)
            .bind(customer_id)
            .execute(&self.write)
            .await
            .map_err(StoreError::backend)?;
        Ok(())
    }

    async fn get_stripe_subscription_id(
        &self,
        tenant: TenantId,
    ) -> Result<Option<String>, StoreError> {
        let row = sqlx::query("SELECT stripe_subscription_id FROM tenants WHERE id = $1")
            .bind(tenant.0 as i64)
            .fetch_optional(&self.read)
            .await
            .map_err(StoreError::backend)?;
        Ok(row.and_then(|r| r.get::<Option<String>, _>("stripe_subscription_id")))
    }

    async fn set_stripe_subscription_id(
        &self,
        tenant: TenantId,
        subscription_id: &str,
    ) -> Result<(), StoreError> {
        sqlx::query("UPDATE tenants SET stripe_subscription_id = $2 WHERE id = $1")
            .bind(tenant.0 as i64)
            .bind(subscription_id)
            .execute(&self.write)
            .await
            .map_err(StoreError::backend)?;
        Ok(())
    }

    async fn find_tenant_by_stripe_customer(
        &self,
        customer_id: &str,
    ) -> Result<Option<TenantId>, StoreError> {
        let row = sqlx::query("SELECT id FROM tenants WHERE stripe_customer_id = $1")
            .bind(customer_id)
            .fetch_optional(&self.read)
            .await
            .map_err(StoreError::backend)?;
        Ok(row.map(|r| TenantId(r.get::<i64, _>("id") as u64)))
    }

    async fn record_stripe_event(
        &self,
        id: &str,
        event_type: &str,
        received_at: u64,
    ) -> Result<bool, StoreError> {
        let res = sqlx::query(
            "INSERT INTO stripe_events (id, type, received_at) VALUES ($1, $2, $3)
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(id)
        .bind(event_type)
        .bind(received_at as i64)
        .execute(&self.write)
        .await
        .map_err(StoreError::backend)?;
        Ok(res.rows_affected() == 1)
    }

    async fn delete_stripe_event(&self, id: &str) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM stripe_events WHERE id = $1")
            .bind(id)
            .execute(&self.write)
            .await
            .map_err(StoreError::backend)?;
        Ok(())
    }
```

- [ ] **Step 5: implementação inerte no LMDB e nos mocks**

Em `src/store/lmdb.rs`, ao lado de `set_tenant_plan` (billing é cloud-only,
mesma razão do plano):

```rust
    async fn get_stripe_customer_id(
        &self,
        _tenant: TenantId,
    ) -> Result<Option<String>, StoreError> {
        Ok(None)
    }

    async fn set_stripe_customer_id(
        &self,
        _tenant: TenantId,
        _customer_id: &str,
    ) -> Result<(), StoreError> {
        Err(StoreError::Unsupported)
    }

    async fn get_stripe_subscription_id(
        &self,
        _tenant: TenantId,
    ) -> Result<Option<String>, StoreError> {
        Ok(None)
    }

    async fn set_stripe_subscription_id(
        &self,
        _tenant: TenantId,
        _subscription_id: &str,
    ) -> Result<(), StoreError> {
        Err(StoreError::Unsupported)
    }

    async fn find_tenant_by_stripe_customer(
        &self,
        _customer_id: &str,
    ) -> Result<Option<TenantId>, StoreError> {
        Ok(None)
    }

    async fn record_stripe_event(
        &self,
        _id: &str,
        _event_type: &str,
        _received_at: u64,
    ) -> Result<bool, StoreError> {
        Err(StoreError::Unsupported)
    }

    async fn delete_stripe_event(&self, _id: &str) -> Result<(), StoreError> {
        Err(StoreError::Unsupported)
    }
```

Copie os mesmos corpos para cada mock encontrado por
`grep -rn "async fn get_tenant_plan" src tests`.

- [ ] **Step 6: rodar e ver passar**

```bash
QUARK_TEST_DATABASE_URL='postgres://quark_test:quark_test@127.0.0.1:5432/quark_test' \
  ~/.cargo/bin/cargo.exe test --test postgres_store_it stripe_billing_columns
```

Esperado: PASSA.

- [ ] **Step 7: gate e commit**

```bash
~/.cargo/bin/cargo.exe fmt
~/.cargo/bin/cargo.exe clippy --all-targets -- -D warnings
~/.cargo/bin/cargo.exe clippy --all-targets --features ee -- -D warnings
git add src/store/ src/domain_router.rs src/webhooks/delivery.rs tests/postgres_store_it.rs
git commit -m "feat(store): vinculos do Stripe como strings opacas (LUC-41)"
```

---

### Task 2: dependências, config e `EeState.billing`

**Files:**
- Modify: `Cargo.toml`
- Create: `src/ee/stripe/mod.rs`
- Modify: `src/ee/mod.rs` (declarar `pub mod stripe;`, campo no `EeState`, boot)
- Modify: `tests/common/mod.rs` (campo `billing: None` no literal de `EeState`)

**Interfaces:**
- Produces: `StripeBilling { client: stripe::Client, webhook_secret: String,
  panel_url: String }`, `StripeBilling::from_env() -> Option<StripeBilling>`,
  `StripeBilling::from_parts(secret_key, webhook_secret, panel_url, api_base:
  Option<&str>) -> Option<StripeBilling>` e
  `EeState.billing: Option<Arc<StripeBilling>>`.

- [ ] **Step 1: dependências pinadas e opcionais**

No `Cargo.toml`, em `[dependencies]` (nome no crates.io leva `async-stripe-`,
o `use` no código é `stripe_*`):

```toml
async-stripe = { version = "=1.0.0-rc.8", default-features = false, features = ["rustls-tls-webpki-roots", "rustls-ring"], optional = true }
async-stripe-core = { version = "=1.0.0-rc.8", features = ["customer"], optional = true }
async-stripe-checkout = { version = "=1.0.0-rc.8", features = ["checkout_session"], optional = true }
async-stripe-billing = { version = "=1.0.0-rc.8", features = ["subscription", "billing_portal_session"], optional = true }
async-stripe-product = { version = "=1.0.0-rc.8", features = ["price"], optional = true }
async-stripe-webhook = { version = "=1.0.0-rc.8", features = ["async-stripe-checkout", "async-stripe-billing"], optional = true }
async-stripe-shared = { version = "=1.0.0-rc.8", optional = true }
async-stripe-types = { version = "=1.0.0-rc.8", optional = true }
```

E na feature `ee` existente em `[features]`, some as deps:

```toml
ee = [
    # ... o que já está lá ...
    "dep:async-stripe",
    "dep:async-stripe-core",
    "dep:async-stripe-checkout",
    "dep:async-stripe-billing",
    "dep:async-stripe-product",
    "dep:async-stripe-webhook",
    "dep:async-stripe-shared",
    "dep:async-stripe-types",
]
```

Confirme com `~/.cargo/bin/cargo.exe build` (sem feature) que o core não
compila nada do Stripe, e com `~/.cargo/bin/cargo.exe build --features ee`
que tudo resolve. Se `rustls-ring` conflitar com o provider que o repo já
usa, troque por `rustls-aws-lc-rs` (a pesquisa não confirmou qual é
obrigatório; o build decide).

- [ ] **Step 2: escrever o teste que falha**

`src/ee/stripe/mod.rs`, comece pelo módulo de teste:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Billing is all-or-nothing: a partially configured env must resolve to
    /// disabled instead of a half-working checkout.
    #[test]
    fn from_parts_requires_all_three_values() {
        assert!(StripeBilling::from_parts("", "whsec_x", "https://app.example.com", None).is_none());
        assert!(StripeBilling::from_parts("sk_test_x", "", "https://app.example.com", None).is_none());
        assert!(StripeBilling::from_parts("sk_test_x", "whsec_x", "", None).is_none());
        let b = StripeBilling::from_parts("sk_test_x", "whsec_x", "https://app.example.com/", None)
            .unwrap();
        // Trailing slash is normalized so URL building can always join with '/'.
        assert_eq!(b.panel_url, "https://app.example.com");
    }
}
```

- [ ] **Step 3: rodar e ver falhar**

```bash
~/.cargo/bin/cargo.exe test --features ee --lib ee::stripe
```

Esperado: FALHA de compilação (`StripeBilling` não existe).

- [ ] **Step 4: escrever a config**

No topo de `src/ee/stripe/mod.rs`:

```rust
//! Stripe billing runtime (LUC-41 phase 2). Covered by `src/ee/LICENSE`.
//!
//! Optional, like the Keycloak runtime: without the three env vars the field
//! stays `None` and the billing endpoints answer 404. A self-hosted
//! Enterprise build without Stripe keeps working in full, which is why the
//! plan layer (phase 1) is independent of the gateway by construction.

pub mod map;

use std::time::Duration;

pub struct StripeBilling {
    pub client: stripe::Client,
    /// The `whsec_...` endpoint secret, for `Webhook::construct_event`.
    pub webhook_secret: String,
    /// Panel base URL without trailing slash, for success/cancel/return URLs.
    pub panel_url: String,
}

impl StripeBilling {
    /// Reads `QUARK_STRIPE_SECRET_KEY`, `QUARK_STRIPE_WEBHOOK_SECRET` and
    /// `QUARK_STRIPE_PANEL_URL`. All three or nothing.
    pub fn from_env() -> Option<StripeBilling> {
        Self::from_parts(
            &std::env::var("QUARK_STRIPE_SECRET_KEY").unwrap_or_default(),
            &std::env::var("QUARK_STRIPE_WEBHOOK_SECRET").unwrap_or_default(),
            &std::env::var("QUARK_STRIPE_PANEL_URL").unwrap_or_default(),
            None,
        )
    }

    /// Explicit parts, mirroring `KeycloakConfig::from_parts` so tests never
    /// mutate process env. `api_base` overrides the API URL for tests that
    /// stand up a local mock server.
    pub fn from_parts(
        secret_key: &str,
        webhook_secret: &str,
        panel_url: &str,
        api_base: Option<&str>,
    ) -> Option<StripeBilling> {
        if secret_key.trim().is_empty()
            || webhook_secret.trim().is_empty()
            || panel_url.trim().is_empty()
        {
            return None;
        }
        let mut builder = stripe::ClientBuilder::new(secret_key.trim())
            .request_strategy(stripe::RequestStrategy::Retry(2))
            .timeout(Duration::from_secs(15));
        if let Some(base) = api_base {
            builder = builder.url(base);
        }
        let client = match builder.build() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "stripe billing disabled: client build failed");
                return None;
            }
        };
        Some(StripeBilling {
            client,
            webhook_secret: webhook_secret.trim().to_string(),
            panel_url: panel_url.trim().trim_end_matches('/').to_string(),
        })
    }
}
```

Crie também um `src/ee/stripe/map.rs` vazio por enquanto (só `//! Plan and
lookup-key mapping (LUC-41 phase 2).`) para o `pub mod map;` compilar; a
Task 3 preenche.

Em `src/ee/mod.rs`: declare `pub mod stripe;` junto dos outros módulos; no
struct `EeState`, junto de `plans`:

```rust
    /// Stripe billing runtime (LUC-41 phase 2), present only when the three
    /// `QUARK_STRIPE_*` env vars are configured. See `stripe::StripeBilling`.
    pub billing: Option<Arc<stripe_mod::StripeBilling>>,
```

Atenção ao nome: o módulo local `crate::ee::stripe` colide com o crate
`stripe` dentro de `src/ee/mod.rs`. Use um alias no topo do arquivo
(`use crate::ee::stripe as stripe_mod;`) ou o caminho completo
`Option<Arc<crate::ee::stripe::StripeBilling>>`; escolha um e seja
consistente. No `boot`, antes do `EeState { ... }` final:

```rust
    let billing = crate::ee::stripe::StripeBilling::from_env().map(Arc::new);
    match &billing {
        Some(_) => tracing::info!("stripe billing enabled"),
        None => tracing::info!(
            "stripe billing: disabled (set QUARK_STRIPE_SECRET_KEY, \
             QUARK_STRIPE_WEBHOOK_SECRET and QUARK_STRIPE_PANEL_URL to enable)"
        ),
    }
```

e o campo `billing,` no literal de retorno. Em `tests/common/mod.rs`, no
literal de `EeState` (linha ~257), some `billing: None,`.

- [ ] **Step 5: rodar e ver passar**

```bash
~/.cargo/bin/cargo.exe test --features ee --lib ee::stripe
```

Esperado: PASSA.

- [ ] **Step 6: gate e commit**

```bash
~/.cargo/bin/cargo.exe fmt
~/.cargo/bin/cargo.exe clippy --all-targets -- -D warnings
~/.cargo/bin/cargo.exe clippy --all-targets --features ee -- -D warnings
git add Cargo.toml Cargo.lock src/ee/ tests/common/mod.rs
git commit -m "feat(ee): runtime de billing Stripe opcional no EeState (LUC-41)"
```

---

### Task 3: lookup keys e mapeamento puro

Tudo função pura, sem IO: é a metade testável sem Stripe de todo o resto.

**Files:**
- Modify: `src/ee/stripe/map.rs`
- Test: unitário inline no próprio arquivo

**Interfaces:**
- Consumes: `Plan` (`crate::ee::plan`, Fase 1),
  `stripe_shared::SubscriptionStatus`.
- Produces: `Cycle` (`Monthly`, `Yearly`),
  `lookup_key(Plan, Cycle) -> Option<&'static str>`,
  `plan_for_lookup_key(&str) -> Option<Plan>`,
  `effective_plan(&SubscriptionStatus, Plan) -> Plan`.

- [ ] **Step 1: escrever o teste que falha**

No fim de `src/ee/stripe/map.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ee::plan::Plan;
    use stripe_shared::SubscriptionStatus;

    /// Every self-service plan/cycle pair has a key, the key round-trips back
    /// to the plan, and non-purchasable plans have none. The keys are the
    /// contract with the dashboard setup in `docs/RUNBOOK-stripe.md`.
    #[test]
    fn lookup_keys_round_trip_for_self_service_plans() {
        for plan in [Plan::Starter, Plan::Pro, Plan::Business] {
            for cycle in [Cycle::Monthly, Cycle::Yearly] {
                let key = lookup_key(plan, cycle).expect("self-service plan has a key");
                assert_eq!(plan_for_lookup_key(key), Some(plan), "{key}");
            }
        }
        assert_eq!(lookup_key(Plan::Free, Cycle::Monthly), None);
        assert_eq!(lookup_key(Plan::Custom, Cycle::Monthly), None);
        assert_eq!(plan_for_lookup_key("nonsense"), None);
    }

    #[test]
    fn published_key_names_are_stable() {
        assert_eq!(lookup_key(Plan::Starter, Cycle::Monthly), Some("starter-monthly"));
        assert_eq!(lookup_key(Plan::Business, Cycle::Yearly), Some("business-yearly"));
    }

    /// The dunning table from the spec (D8): retries keep the paid plan,
    /// terminal states drop to Free.
    #[test]
    fn effective_plan_follows_the_dunning_table() {
        let paid = Plan::Pro;
        for keeps in [
            SubscriptionStatus::Active,
            SubscriptionStatus::Trialing,
            SubscriptionStatus::PastDue,
        ] {
            assert_eq!(effective_plan(&keeps, paid), Plan::Pro, "{keeps:?}");
        }
        for drops in [
            SubscriptionStatus::Canceled,
            SubscriptionStatus::Unpaid,
            SubscriptionStatus::IncompleteExpired,
            SubscriptionStatus::Paused,
            SubscriptionStatus::Incomplete,
        ] {
            assert_eq!(effective_plan(&drops, paid), Plan::Free, "{drops:?}");
        }
    }
}
```

- [ ] **Step 2: rodar e ver falhar**

```bash
~/.cargo/bin/cargo.exe test --features ee --lib ee::stripe::map
```

Esperado: FALHA de compilação (`Cycle` não existe).

- [ ] **Step 3: escrever o mapeamento**

No topo de `src/ee/stripe/map.rs`, acima do módulo de teste:

```rust
//! Plan and lookup-key mapping (LUC-41 phase 2). Pure functions, no IO.
//!
//! Prices in Stripe carry a stable `lookup_key`; price IDs never appear in
//! code or env (spec D3). These names are the contract with the dashboard
//! setup documented in `docs/RUNBOOK-stripe.md`.

use crate::ee::plan::Plan;
use stripe_shared::SubscriptionStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cycle {
    Monthly,
    Yearly,
}

impl Cycle {
    /// Wire name used in the checkout request body. Named `parse`, not
    /// `from_str`, to keep clippy's `should_implement_trait` quiet without
    /// pulling in a `FromStr` impl nothing needs.
    pub fn parse(s: &str) -> Option<Cycle> {
        match s {
            "monthly" => Some(Cycle::Monthly),
            "yearly" => Some(Cycle::Yearly),
            _ => None,
        }
    }
}

/// The Stripe price lookup key for a plan and cycle. `None` for plans that
/// are not self-service: Free has nothing to buy, Custom is negotiated and
/// set through the operator escape hatch (phase 1).
pub fn lookup_key(plan: Plan, cycle: Cycle) -> Option<&'static str> {
    let key = match (plan, cycle) {
        (Plan::Starter, Cycle::Monthly) => "starter-monthly",
        (Plan::Starter, Cycle::Yearly) => "starter-yearly",
        (Plan::Pro, Cycle::Monthly) => "pro-monthly",
        (Plan::Pro, Cycle::Yearly) => "pro-yearly",
        (Plan::Business, Cycle::Monthly) => "business-monthly",
        (Plan::Business, Cycle::Yearly) => "business-yearly",
        (Plan::Free, _) | (Plan::Custom, _) => return None,
    };
    Some(key)
}

/// Inverts `lookup_key`, cycle-insensitive: the webhook only needs the plan.
pub fn plan_for_lookup_key(key: &str) -> Option<Plan> {
    match key {
        "starter-monthly" | "starter-yearly" => Some(Plan::Starter),
        "pro-monthly" | "pro-yearly" => Some(Plan::Pro),
        "business-monthly" | "business-yearly" => Some(Plan::Business),
        _ => None,
    }
}

/// The dunning table from the spec (D8): what plan a subscription status
/// actually grants. `past_due` keeps the paid plan through the Smart Retries
/// window; every terminal state drops to Free.
pub fn effective_plan(status: &SubscriptionStatus, paid: Plan) -> Plan {
    match status {
        SubscriptionStatus::Active
        | SubscriptionStatus::Trialing
        | SubscriptionStatus::PastDue => paid,
        _ => Plan::Free,
    }
}
```

- [ ] **Step 4: rodar e ver passar**

```bash
~/.cargo/bin/cargo.exe test --features ee --lib ee::stripe::map
```

Esperado: PASSA, 3 testes.

- [ ] **Step 5: commit**

```bash
~/.cargo/bin/cargo.exe fmt
git add src/ee/stripe/map.rs
git commit -m "feat(ee): lookup keys e tabela de dunning (LUC-41)"
```

---

### Task 4: endpoints de checkout e portal

Owner-only via sessão (mesmo padrão de `admin_tenants_delete`): token de API
não carrega role, então billing é operação de navegador logado.

**Files:**
- Create: `src/ee/api/billing.rs`
- Modify: `src/ee/api/mod.rs` (declarar `mod billing;`,
  `pub(crate) use billing::*;`, montar as rotas)
- Test: `tests/billing_it.rs`

**Interfaces:**
- Consumes: `StripeBilling` (Task 2), `lookup_key`/`Cycle` (Task 3),
  `current_session` (`src/ee/api/mod.rs`), `Store` (Task 1),
  `Plan::from_stored` (Fase 1).
- Produces: `POST /admin/billing/checkout`, `POST /admin/billing/portal`,
  e `require_owner(&AppState, &HeaderMap) -> Result<(u64, TenantId), StatusCode>`
  (usado também pelo webhook? não: o webhook não tem sessão; só estes dois).

- [ ] **Step 1: escrever o teste que falha**

Crie `tests/billing_it.rs`:

```rust
// Codigo de teste pode entrar em panico: a falha e o proprio sinal.
#![allow(clippy::unwrap_used)]

// Enterprise suite: billing only exists in the `--features ee` build (LUC-41).
#![cfg(feature = "ee")]

use quark::tenant::{Role, Tenant, TenantId};
use tower::ServiceExt;

mod common;

const PANEL: &str = "https://app.example.com";

/// State with a Postgres store, multi-tenant on, and billing configured
/// against `api_base` (a local mock, or any unreachable port for tests that
/// never get that far).
async fn state_with_billing(
    api_base: &str,
) -> (std::sync::Arc<quark::api::AppState>, TenantId) {
    let url = std::env::var("QUARK_TEST_DATABASE_URL").expect("gated test");
    let store = quark::store::postgres::PostgresStore::open(&url, true)
        .await
        .unwrap();
    store.reset_for_tests().await.unwrap();
    let t = TenantId(7100);
    store
        .put_tenant(&Tenant {
            id: t,
            name: "Acme".into(),
            slug: "acme-billing".into(),
            created: 0,
        })
        .await
        .unwrap();
    let sink = common::test_sink();
    let billing = quark::ee::stripe::StripeBilling::from_parts(
        "sk_test_x",
        "whsec_test",
        PANEL,
        Some(api_base),
    )
    .unwrap();
    let st = common::TestState::new(std::sync::Arc::new(store), sink)
        .multi_tenant(true)
        .billing(Some(std::sync::Arc::new(billing)))
        .build();
    (st, t)
}

/// Seeds a user, a membership with `role`, and a session cookie for it.
/// Returns the Cookie header value.
async fn seed_session(
    st: &quark::api::AppState,
    tenant: TenantId,
    user_id: u64,
    role: Role,
) -> String {
    let raw = format!("billing-session-{user_id}");
    st.store
        .put_membership(&quark::tenant::Membership {
            user_id,
            tenant_id: tenant,
            role,
        })
        .await
        .unwrap();
    st.store
        .put_session(
            tenant,
            &quark::auth::Session {
                token_hash: quark::auth::hash_token(&raw),
                subject: format!("sub-{user_id}"),
                display: "user".into(),
                scopes: vec!["links:write".into()],
                created: 0,
                expires: u64::MAX,
                tenant_id: tenant,
                user_id,
                id_token: None,
            },
        )
        .await
        .unwrap();
    format!("qk_session={raw}")
}

fn post(uri: &str, cookie: Option<&str>, body: serde_json::Value) -> axum::http::Request<axum::body::Body> {
    let mut b = axum::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(c) = cookie {
        b = b.header("cookie", c);
    }
    b.body(axum::body::Body::from(body.to_string())).unwrap()
}

#[tokio::test]
#[serial_test::file_serial]
async fn checkout_is_404_when_billing_is_not_configured() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    let url = std::env::var("QUARK_TEST_DATABASE_URL").unwrap();
    let store = quark::store::postgres::PostgresStore::open(&url, true)
        .await
        .unwrap();
    store.reset_for_tests().await.unwrap();
    let sink = common::test_sink();
    let st = common::TestState::new(std::sync::Arc::new(store), sink)
        .multi_tenant(true)
        .build(); // no billing
    let app = quark::api::router(st);
    let res = app
        .oneshot(post(
            "/admin/billing/checkout",
            None,
            serde_json::json!({"plan": "pro", "cycle": "monthly", "currency": "usd"}),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
#[serial_test::file_serial]
async fn checkout_requires_a_session_and_the_owner_role() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    // Unreachable api_base: these requests must be rejected before any Stripe
    // call is attempted.
    let (st, t) = state_with_billing("http://127.0.0.1:9").await;
    let app = quark::api::router(st.clone());
    let body = serde_json::json!({"plan": "pro", "cycle": "monthly", "currency": "usd"});

    // No session: 401.
    let res = app
        .clone()
        .oneshot(post("/admin/billing/checkout", None, body.clone()))
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::UNAUTHORIZED);

    // Admin (not Owner): 403.
    let admin_cookie = seed_session(&st, t, 21, Role::Admin).await;
    let res = app
        .clone()
        .oneshot(post("/admin/billing/checkout", Some(&admin_cookie), body.clone()))
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::FORBIDDEN);

    // Owner with a nonsense plan: 400 before touching Stripe.
    let owner_cookie = seed_session(&st, t, 22, Role::Owner).await;
    let res = app
        .clone()
        .oneshot(post(
            "/admin/billing/checkout",
            Some(&owner_cookie),
            serde_json::json!({"plan": "free", "cycle": "monthly", "currency": "usd"}),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[serial_test::file_serial]
async fn portal_is_404_without_a_stripe_customer() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    let (st, t) = state_with_billing("http://127.0.0.1:9").await;
    let app = quark::api::router(st.clone());
    let owner_cookie = seed_session(&st, t, 23, Role::Owner).await;
    let res = app
        .oneshot(post("/admin/billing/portal", Some(&owner_cookie), serde_json::json!({})))
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::NOT_FOUND);
}
```

Antes de rodar: confira os nomes reais de `Membership` (campos `user_id`,
`tenant_id`, `role`) em `src/tenant.rs` e de `Session` em `src/auth.rs`, e o
nome do cookie de sessão (`qk_session`, constante `SESSION_COOKIE` em
`src/api/`); ajuste o teste ao que existe. O builder de `tests/common` ainda
não tem o setter `.billing(...)`: crie-o nesta task, ao lado dos setters
existentes (mesmo padrão fluente), gated `#[cfg(feature = "ee")]`.
A sessão só resolve com `oidc_configured` ligado (veja
`current_session`); se o builder tiver um setter para isso
(`tests/common/mod.rs`), ligue-o no `state_with_billing`; sem sessão
resolvida o teste veria 401 em tudo.

- [ ] **Step 2: rodar e ver falhar**

```bash
QUARK_TEST_DATABASE_URL='postgres://quark_test:quark_test@127.0.0.1:5432/quark_test' \
  ~/.cargo/bin/cargo.exe test --features ee --test billing_it
```

Esperado: FALHA (404 em tudo: as rotas não existem; e o setter `.billing`
não compila até você criá-lo).

- [ ] **Step 3: escrever os handlers**

`src/ee/api/billing.rs`:

```rust
//! Billing endpoints (LUC-41 phase 2). Covered by `src/ee/LICENSE`.
//!
//! Checkout and portal are Owner-only, resolved from the session like
//! `admin_tenants_delete`: an API token carries scopes, not a role, so
//! billing is a logged-in-browser operation. Neither endpoint writes `plan`;
//! that is the webhook's job (spec D4).

use super::*;
use crate::ee::plan::Plan;
use crate::ee::stripe::map::{lookup_key, Cycle};

/// Resolves the session and requires the Owner role on its workspace.
/// 401 without a session, 403 with one that is not Owner, 503 on store error.
pub(super) async fn require_owner(
    st: &AppState,
    headers: &HeaderMap,
) -> Result<(u64, crate::tenant::TenantId), StatusCode> {
    let Some(session) = current_session(st, headers).await else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    match st.store.get_membership(session.user_id, session.tenant_id).await {
        Ok(Some(m)) if m.role == crate::tenant::Role::Owner => {
            Ok((session.user_id, session.tenant_id))
        }
        Ok(Some(_)) => Err(StatusCode::FORBIDDEN),
        Ok(None) => Err(StatusCode::UNAUTHORIZED),
        Err(_) => Err(StatusCode::SERVICE_UNAVAILABLE),
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct CheckoutReq {
    pub plan: String,
    pub cycle: String,
    pub currency: String,
}

/// `POST /admin/billing/checkout`: creates (or reuses) the tenant's Stripe
/// customer and answers the hosted Checkout URL.
pub(crate) async fn admin_billing_checkout(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CheckoutReq>,
) -> Response {
    if !st.multi_tenant {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(billing) = st.ee.billing.clone() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let (_user, tenant) = match require_owner(&st, &headers).await {
        Ok(v) => v,
        Err(status) => return status.into_response(),
    };

    // Validate the request before any Stripe call.
    let plan = Plan::from_stored(&req.plan);
    if plan.as_str() != req.plan {
        return (StatusCode::BAD_REQUEST, "unknown plan").into_response();
    }
    let Some(cycle) = Cycle::parse(&req.cycle) else {
        return (StatusCode::BAD_REQUEST, "cycle must be monthly or yearly").into_response();
    };
    let Some(key) = lookup_key(plan, cycle) else {
        return (StatusCode::BAD_REQUEST, "plan is not self-service").into_response();
    };
    // Spec D5: the currency is a first-checkout decision and Stripe locks it
    // on the customer afterwards, so it is explicit here, never IP-guessed.
    let currency = match req.currency.as_str() {
        "usd" => stripe_types::Currency::USD,
        "brl" => stripe_types::Currency::BRL,
        _ => return (StatusCode::BAD_REQUEST, "currency must be usd or brl").into_response(),
    };

    // Customer: reuse or create-and-persist.
    let customer_id = match st.store.get_stripe_customer_id(tenant).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            let tenant_row = match st.store.get_tenant(tenant).await {
                Ok(Some(t)) => t,
                Ok(None) => return StatusCode::NOT_FOUND.into_response(),
                Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
            };
            let created = stripe_core::customer::CreateCustomer::new()
                .name(tenant_row.name.as_str())
                .metadata([(String::from("tenant_id"), tenant.0.to_string())])
                .send(&billing.client)
                .await;
            let customer = match created {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(error = %e, tenant_id = tenant.0, "stripe customer create failed");
                    return StatusCode::SERVICE_UNAVAILABLE.into_response();
                }
            };
            if st
                .store
                .set_stripe_customer_id(tenant, customer.id.as_str())
                .await
                .is_err()
            {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            customer.id.to_string()
        }
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };

    // Resolve the price by lookup key (spec D3: no price IDs in code or env).
    let prices = match stripe_product::price::ListPrice::new()
        .lookup_keys(vec![key.to_string()])
        .active(true)
        .send(&billing.client)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "stripe price list failed");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    };
    let Some(price) = prices.data.first() else {
        // The dashboard is missing a price for this key: an operator problem,
        // not a caller problem. The runbook documents the keys.
        tracing::error!(lookup_key = key, "no active stripe price for lookup key");
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };

    // Trial once per tenant (spec D6): a tenant that ever had a subscription
    // does not get another trial by resubscribing.
    let had_subscription = match st.store.get_stripe_subscription_id(tenant).await {
        Ok(v) => v.is_some(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };

    use stripe_checkout::checkout_session::{
        CreateCheckoutSession, CreateCheckoutSessionLineItems,
        CreateCheckoutSessionSubscriptionData,
        CreateCheckoutSessionSubscriptionDataTrialSettings,
        CreateCheckoutSessionSubscriptionDataTrialSettingsEndBehavior,
        CreateCheckoutSessionSubscriptionDataTrialSettingsEndBehaviorMissingPaymentMethod,
    };
    let mut sub_data = CreateCheckoutSessionSubscriptionData {
        metadata: Some([(String::from("tenant_id"), tenant.0.to_string())].into()),
        ..Default::default()
    };
    if !had_subscription {
        sub_data.trial_period_days = Some(14);
        sub_data.trial_settings = Some(CreateCheckoutSessionSubscriptionDataTrialSettings::new(
            CreateCheckoutSessionSubscriptionDataTrialSettingsEndBehavior::new(
                CreateCheckoutSessionSubscriptionDataTrialSettingsEndBehaviorMissingPaymentMethod::Cancel,
            ),
        ));
    }
    let session = match CreateCheckoutSession::new()
        .mode(stripe_checkout::CheckoutSessionMode::Subscription)
        .customer(customer_id.as_str())
        .client_reference_id(tenant.0.to_string())
        .currency(currency)
        .line_items(vec![CreateCheckoutSessionLineItems {
            price: Some(price.id.to_string()),
            quantity: Some(1),
            ..Default::default()
        }])
        .subscription_data(sub_data)
        .success_url(format!("{}/settings/billing?checkout=success", billing.panel_url))
        .cancel_url(format!("{}/settings/billing?checkout=cancel", billing.panel_url))
        .send(&billing.client)
        .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, tenant_id = tenant.0, "stripe checkout session failed");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    };
    match session.url {
        Some(url) => Json(serde_json::json!({ "url": url })).into_response(),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// `POST /admin/billing/portal`: hosted Customer Portal session. 404 while
/// the tenant has no Stripe customer (nothing to manage yet).
pub(crate) async fn admin_billing_portal(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if !st.multi_tenant {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(billing) = st.ee.billing.clone() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let (_user, tenant) = match require_owner(&st, &headers).await {
        Ok(v) => v,
        Err(status) => return status.into_response(),
    };
    let customer_id = match st.store.get_stripe_customer_id(tenant).await {
        Ok(Some(id)) => id,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let portal = match stripe_billing::billing_portal_session::CreateBillingPortalSession::new()
        .customer(customer_id.as_str())
        .return_url(format!("{}/settings/billing", billing.panel_url))
        .send(&billing.client)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, tenant_id = tenant.0, "stripe portal session failed");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    };
    Json(serde_json::json!({ "url": portal.url })).into_response()
}
```

Em `src/ee/api/mod.rs`: `mod billing;` junto dos outros,
`pub(crate) use billing::*;`, e no `mount`, depois das rotas de plan:

```rust
        .route("/admin/billing/checkout", post(admin_billing_checkout))
        .route("/admin/billing/portal", post(admin_billing_portal))
```

- [ ] **Step 4: rodar e ver passar**

```bash
QUARK_TEST_DATABASE_URL='postgres://quark_test:quark_test@127.0.0.1:5432/quark_test' \
  ~/.cargo/bin/cargo.exe test --features ee --test billing_it
```

Esperado: PASSA, 3 testes.

- [ ] **Step 5: gate e commit**

```bash
~/.cargo/bin/cargo.exe fmt
~/.cargo/bin/cargo.exe clippy --all-targets -- -D warnings
~/.cargo/bin/cargo.exe clippy --all-targets --features ee -- -D warnings
git add src/ee/api/ tests/billing_it.rs tests/common/mod.rs
git commit -m "feat(ee): checkout e portal do Stripe, Owner-only (LUC-41)"
```

---

### Task 5: webhook idempotente que escreve o plano

O único escritor automático de `plan`. Verifica assinatura, deduplica por
event id, e para estado de assinatura busca a subscription na API (nunca
confia no payload, spec D4).

**Files:**
- Modify: `src/ee/api/billing.rs` (handler + applier)
- Modify: `src/ee/api/mod.rs` (rota `/stripe/webhook`)
- Test: `tests/billing_it.rs`

**Interfaces:**
- Consumes: `StripeBilling.webhook_secret`, `record_stripe_event`,
  `find_tenant_by_stripe_customer`, `set_stripe_subscription_id`,
  `set_tenant_plan` (Fase 1), `PlanCache::invalidate` (Fase 1),
  `plan_for_lookup_key`/`effective_plan` (Task 3).
- Produces: `POST /stripe/webhook` e
  `apply_subscription(&AppState, &stripe_shared::Subscription) -> Result<(), StatusCode>`.

- [ ] **Step 1: escrever o teste que falha**

Acrescente a `tests/billing_it.rs`. O crate `stripe_webhook` expõe
`Webhook::generate_test_header(payload, secret, Option<i64>)`, que gera o
header `stripe-signature` válido; com `None` usa o timestamp corrente.

```rust
fn webhook_post(payload: &str, secret: &str) -> axum::http::Request<axum::body::Body> {
    let sig = stripe_webhook::Webhook::generate_test_header(payload, secret, None);
    axum::http::Request::builder()
        .method("POST")
        .uri("/stripe/webhook")
        .header("stripe-signature", sig)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(payload.to_string()))
        .unwrap()
}

/// Minimal `checkout.session.completed` event. If deserialization fails,
/// serde names the missing field: complete the fixture, do not weaken the
/// handler.
fn checkout_completed_event(event_id: &str, tenant: TenantId) -> String {
    serde_json::json!({
        "id": event_id,
        "object": "event",
        "api_version": "2026-07-29.dahlia",
        "created": 1700000000,
        "livemode": false,
        "pending_webhooks": 0,
        "type": "checkout.session.completed",
        "data": {
            "object": {
                "id": "cs_test_1",
                "object": "checkout.session",
                "mode": "subscription",
                "status": "complete",
                "client_reference_id": tenant.0.to_string(),
                "customer": "cus_123",
                "subscription": "sub_123",
                "livemode": false,
                "created": 1700000000
            }
        }
    })
    .to_string()
}

#[tokio::test]
#[serial_test::file_serial]
async fn webhook_rejects_a_bad_signature() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    let (st, t) = state_with_billing("http://127.0.0.1:9").await;
    let app = quark::api::router(st);
    let payload = checkout_completed_event("evt_sig", t);
    // Signed with the WRONG secret.
    let req = webhook_post(&payload, "whsec_wrong");
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[serial_test::file_serial]
async fn webhook_records_the_subscription_and_deduplicates() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    let (st, t) = state_with_billing("http://127.0.0.1:9").await;
    st.store.set_stripe_customer_id(t, "cus_123").await.unwrap();
    let app = quark::api::router(st.clone());
    let payload = checkout_completed_event("evt_dup", t);

    let res = app.clone().oneshot(webhook_post(&payload, "whsec_test")).await.unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::OK);
    assert_eq!(
        st.store.get_stripe_subscription_id(t).await.unwrap().as_deref(),
        Some("sub_123")
    );

    // Same event id again: 200, no effect, and no crash.
    let res = app.oneshot(webhook_post(&payload, "whsec_test")).await.unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::OK);
}

/// The applier is exercised directly with a subscription deserialized from a
/// fixture: the endpoint-level path for subscription events needs a live (or
/// mocked) Stripe API for the mandatory re-fetch, which the sandbox runbook
/// covers manually.
#[tokio::test]
#[serial_test::file_serial]
async fn apply_subscription_maps_status_and_lookup_key_to_the_plan() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    let (st, t) = state_with_billing("http://127.0.0.1:9").await;
    st.store.set_stripe_customer_id(t, "cus_123").await.unwrap();

    let sub_json = serde_json::json!({
        "id": "sub_123",
        "object": "subscription",
        "status": "active",
        "customer": "cus_123",
        "cancel_at_period_end": false,
        "created": 1700000000,
        "currency": "usd",
        "livemode": false,
        "metadata": {"tenant_id": t.0.to_string()},
        "items": {
            "object": "list",
            "url": "/v1/subscription_items?subscription=sub_123",
            "has_more": false,
            "data": [{
                "id": "si_1",
                "object": "subscription_item",
                "created": 1700000000,
                "metadata": {},
                "quantity": 1,
                "subscription": "sub_123",
                "price": {
                    "id": "price_1",
                    "object": "price",
                    "active": true,
                    "created": 1700000000,
                    "currency": "usd",
                    "livemode": false,
                    "lookup_key": "pro-monthly",
                    "metadata": {},
                    "product": "prod_1",
                    "type": "recurring"
                }
            }]
        }
    });
    let sub: stripe_shared::Subscription = serde_json::from_value(sub_json).unwrap();
    quark::ee::api::apply_subscription(&st, &sub).await.unwrap();
    assert_eq!(
        st.store.get_tenant_plan(t).await.unwrap().as_deref(),
        Some("pro")
    );

    // Terminal status drops the tenant to free.
    let mut canceled = sub;
    canceled.status = stripe_shared::SubscriptionStatus::Canceled;
    quark::ee::api::apply_subscription(&st, &canceled).await.unwrap();
    assert_eq!(
        st.store.get_tenant_plan(t).await.unwrap().as_deref(),
        Some("free")
    );
}
```

Os dois fixtures são mínimos de propósito. Se `serde_json::from_value` ou o
parse do evento falhar por campo obrigatório faltando, o erro nomeia o campo:
adicione-o ao fixture com um valor plausível. Nunca afrouxe o handler para o
fixture passar. Para `apply_subscription` ser chamável do teste, exporte-o no
`pub use` de `src/ee/api/mod.rs` (os testes importam `quark::ee::api::...`).

- [ ] **Step 2: rodar e ver falhar**

```bash
QUARK_TEST_DATABASE_URL='postgres://quark_test:quark_test@127.0.0.1:5432/quark_test' \
  ~/.cargo/bin/cargo.exe test --features ee --test billing_it webhook
```

Esperado: FALHA (rota não existe, `apply_subscription` não existe).

- [ ] **Step 3: escrever o handler e o applier**

No fim de `src/ee/api/billing.rs`:

```rust
/// Applies a subscription's current state to its tenant: status plus the
/// price lookup key decide the effective plan (spec D8), written through the
/// phase 1 seam and cache. Public to the crate's tests; the only production
/// caller is the webhook below.
pub async fn apply_subscription(
    st: &AppState,
    sub: &stripe_shared::Subscription,
) -> Result<(), StatusCode> {
    // The webhook arrives with a customer id; metadata's tenant_id is the
    // fallback for events older than the reverse index.
    let customer_id = match &sub.customer {
        stripe_types::Expandable::Id(id) => id.to_string(),
        stripe_types::Expandable::Object(c) => c.id.to_string(),
    };
    let tenant = match st.store.find_tenant_by_stripe_customer(&customer_id).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            match sub
                .metadata
                .get("tenant_id")
                .and_then(|v| v.parse::<u64>().ok())
            {
                Some(id) => crate::tenant::TenantId(id),
                None => {
                    // Orphan event (an old environment, a deleted tenant):
                    // acknowledge, never leave it retrying forever.
                    tracing::warn!(customer = %customer_id, "stripe event for unknown tenant");
                    return Ok(());
                }
            }
        }
        Err(_) => return Err(StatusCode::SERVICE_UNAVAILABLE),
    };
    let paid = sub
        .items
        .data
        .first()
        .and_then(|item| item.price.lookup_key.as_deref())
        .and_then(crate::ee::stripe::map::plan_for_lookup_key)
        .unwrap_or(crate::ee::plan::Plan::Free);
    let effective = crate::ee::stripe::map::effective_plan(&sub.status, paid);
    if st
        .store
        .set_tenant_plan(tenant, effective.as_str())
        .await
        .is_err()
    {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    if st
        .store
        .set_stripe_subscription_id(tenant, sub.id.as_str())
        .await
        .is_err()
    {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    st.ee.plans.invalidate(tenant).await;
    tracing::info!(
        tenant_id = tenant.0,
        plan = effective.as_str(),
        status = ?sub.status,
        "plan updated from stripe subscription"
    );
    Ok(())
}

/// `POST /stripe/webhook`. Public route: the authentication IS the
/// `Stripe-Signature` header. 400 on a bad signature, 200 on a duplicate,
/// 5xx on our own failure so Stripe retries.
pub(crate) async fn stripe_webhook(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let Some(billing) = st.ee.billing.clone() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(sig) = headers.get("stripe-signature").and_then(|v| v.to_str().ok()) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let event = match stripe_webhook::Webhook::construct_event(
        &body,
        sig,
        &billing.webhook_secret,
    ) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(error = %e, "stripe webhook signature rejected");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    // Idempotency ledger (spec D4): one row per event id, replays are acked
    // and skipped. The row is RELEASED again on every 5xx below: otherwise
    // Stripe's retry of a failed delivery would hit the dedup and the change
    // would be lost for good.
    let event_id = event.id.to_string();
    match st
        .store
        .record_stripe_event(&event_id, event.type_.as_str(), now())
        .await
    {
        Ok(true) => {}
        Ok(false) => return StatusCode::OK.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
    // Best-effort ledger release before answering 5xx (see above).
    let fail = |st: Arc<AppState>, event_id: String, status: StatusCode| async move {
        let _ = st.store.delete_stripe_event(&event_id).await;
        status.into_response()
    };

    use stripe_webhook::EventObject;
    match event.data.object {
        EventObject::CheckoutSessionCompleted(session) => {
            // The subscription id is all this event contributes; the plan
            // itself arrives via customer.subscription.created/updated.
            let tenant = session
                .client_reference_id
                .as_deref()
                .and_then(|v| v.parse::<u64>().ok())
                .map(crate::tenant::TenantId);
            let sub_id = session.subscription.as_ref().map(|s| match s {
                stripe_types::Expandable::Id(id) => id.to_string(),
                stripe_types::Expandable::Object(o) => o.id.to_string(),
            });
            if let (Some(tenant), Some(sub_id)) = (tenant, sub_id) {
                if st
                    .store
                    .set_stripe_subscription_id(tenant, &sub_id)
                    .await
                    .is_err()
                {
                    return fail(st, event_id, StatusCode::SERVICE_UNAVAILABLE).await;
                }
            }
            StatusCode::OK.into_response()
        }
        EventObject::CustomerSubscriptionCreated(sub)
        | EventObject::CustomerSubscriptionUpdated(sub)
        | EventObject::CustomerSubscriptionDeleted(sub) => {
            // Spec D4: never trust the payload's state, fetch the current
            // subscription. Event order is not guaranteed; the API is.
            let fetched = stripe_billing::subscription::RetrieveSubscription::new(
                sub.id.clone(),
            )
            .send(&billing.client)
            .await;
            let current = match fetched {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(error = %e, "stripe subscription re-fetch failed");
                    return fail(st, event_id, StatusCode::SERVICE_UNAVAILABLE).await;
                }
            };
            match apply_subscription(&st, &current).await {
                Ok(()) => StatusCode::OK.into_response(),
                Err(status) => fail(st, event_id, status).await,
            }
        }
        _ => {
            // invoice.paid / invoice.payment_failed and anything else we do
            // not act on: structured log only, Stripe's own emails handle
            // dunning communication.
            tracing::info!(event_type = %event.type_, "stripe event acknowledged");
            StatusCode::OK.into_response()
        }
    }
}
```

Confirme no docs.rs de `stripe_webhook` (1.0.0-rc.8) os nomes exatos das
variantes de `EventObject` (`CheckoutSessionCompleted` está confirmado no
exemplo oficial; `CustomerSubscriptionCreated/Updated/Deleted` seguem o mesmo
padrão de nome do event type). Confirme também se `event.type_.as_str()`
existe; senão use `event.type_.to_string()` e ajuste `record_stripe_event`.
Se `RetrieveSubscription::new` exigir `SubscriptionId` em vez de aceitar o
id clonado, converta com o tipo que o docs.rs indicar.

Em `src/ee/api/mod.rs`, no `mount`:

```rust
        .route("/stripe/webhook", post(stripe_webhook))
```

E exponha o applier para os testes: `pub use billing::apply_subscription;`.

Nota sobre LUC-85 (admin restrito ao host do backend): a rota é `/stripe/*`,
não `/admin/*`, então o guard de host não a bloqueia; confirme rodando o
teste, e se algum middleware de host barrar, trate `/stripe/webhook` como o
`/` público é tratado.

- [ ] **Step 4: rodar e ver passar**

```bash
QUARK_TEST_DATABASE_URL='postgres://quark_test:quark_test@127.0.0.1:5432/quark_test' \
  ~/.cargo/bin/cargo.exe test --features ee --test billing_it
```

Esperado: PASSA, 6 testes.

- [ ] **Step 5: gate e commit**

```bash
~/.cargo/bin/cargo.exe fmt
~/.cargo/bin/cargo.exe clippy --all-targets -- -D warnings
~/.cargo/bin/cargo.exe clippy --all-targets --features ee -- -D warnings
git add src/ee/api/ tests/billing_it.rs
git commit -m "feat(ee): webhook do Stripe escreve o plano, idempotente (LUC-41)"
```

---

### Task 6: docs, runbook e gates finais

**Files:**
- Create: `docs/BILLING.md`, `docs/BILLING.PT_BR.md`
- Create: `docs/RUNBOOK-stripe.md`
- Modify: `docs/LICENSING.md` e `docs/LICENSING.PT_BR.md` (uma linha na seção
  de variáveis Enterprise apontando `docs/BILLING.md`)
- Modify: `docs/PLANS.md` e `docs/PLANS.PT_BR.md` (a seção "como o operador
  troca o plano" ganha a frase: com Stripe configurado a troca acontece via
  assinatura; o endpoint do operador vira escape hatch)

- [ ] **Step 1: escrever `docs/BILLING.md` e o twin**

Cabeçalho de troca de idioma (`**English** · [Português](BILLING.PT_BR.md)` e
o espelho). Conteúdo, em prosa direta: como um Owner assina (checkout
hospedado, moeda decidida no primeiro checkout e travada, trial de 14 dias
sem cartão uma única vez por workspace), o que o Customer Portal resolve
(upgrade, downgrade, cancelamento, cartão, faturas), a tabela de estados
(active/trialing/past_due mantêm o plano; canceled/unpaid/incomplete_expired/
paused rebaixam para Free), o que acontece num downgrade com recursos acima
do teto (nada é apagado, criação nova é bloqueada), e que a edição Community
não tem billing nenhum. As três env vars (`QUARK_STRIPE_SECRET_KEY`,
`QUARK_STRIPE_WEBHOOK_SECRET`, `QUARK_STRIPE_PANEL_URL`) e o comportamento
sem elas (endpoints 404).

- [ ] **Step 2: escrever `docs/RUNBOOK-stripe.md`**

Runbook do operador, seguindo o formato dos runbooks existentes
(`docs/RUNBOOK-keycloak-p2e.md`). Seções: criar os 6 products/prices com as
lookup keys exatas (`starter-monthly`, `starter-yearly`, `pro-monthly`,
`pro-yearly`, `business-monthly`, `business-yearly`, cada um com preço USD e
BRL no mesmo price, multi-currency); configurar o Customer Portal (restringir
a troca aos 3 products, desligar "update quantities", downgrade no fim do
período); criar o webhook endpoint (`https://<backend>/stripe/webhook`) com
os 6 eventos (`checkout.session.completed`,
`customer.subscription.created`, `customer.subscription.updated`,
`customer.subscription.deleted`, `invoice.paid`, `invoice.payment_failed`) e
copiar o `whsec`; ligar Smart Retries e os e-mails de recuperação; teste
local com `stripe listen --forward-to localhost:8080/stripe/webhook`; test
clocks no sandbox para validar renovação e dunning; e a nota fiscal (sem
Stripe Tax para merchant BR, NFS-e/ISS é processo fora do Stripe).

- [ ] **Step 3: gate completo**

```bash
~/.cargo/bin/cargo.exe fmt
~/.cargo/bin/cargo.exe clippy --all-targets -- -D warnings
~/.cargo/bin/cargo.exe clippy --all-targets --features ee -- -D warnings
QUARK_TEST_DATABASE_URL='postgres://quark_test:quark_test@127.0.0.1:5432/quark_test' \
  ~/.cargo/bin/cargo.exe test --no-fail-fast
QUARK_TEST_DATABASE_URL='postgres://quark_test:quark_test@127.0.0.1:5432/quark_test' \
  ~/.cargo/bin/cargo.exe test --features ee --no-fail-fast
```

Esperado: zero falhas nos dois modos.

- [ ] **Step 4: provar a separação open-core**

```bash
rm -rf src/ee web/src/ee
~/.cargo/bin/cargo.exe build
~/.cargo/bin/cargo.exe test --no-fail-fast
git checkout -- src/ee web/src/ee
```

Esperado: compila e passa sem as pastas. Os crates do Stripe não podem nem
ser baixados no build sem `--features ee` (são `optional`).

- [ ] **Step 5: provar que o redirect não toca billing**

```bash
grep -n "stripe\|billing" src/api/links.rs src/domain_router.rs src/cache/mod.rs
```

Esperado: nenhuma linha de produção (o mock de teste do `domain_router.rs`
implementa os métodos do trait com corpos inertes; isso é teste, não caminho
de redirect, e é aceitável). Nenhuma chamada de entitlement ou billing no
caminho de redirect.

- [ ] **Step 6: commit final**

```bash
git add -A
git commit -m "docs(ee): billing do Stripe e runbook do dashboard (LUC-41)"
```

---

## Fora deste plano

Tela de billing no painel e tratamento do 402 com CTA de upgrade (front,
espera a landing). Custom domain do checkout (espera LUC-147). Soft cap,
contador mensal e teto de automação (Fase 3). Stripe Tax. Troca de moeda de
um customer existente (operação manual do operador, documentada como não
suportada). E-mail próprio de aviso de dunning (os automáticos do Stripe
cobrem o lançamento).
