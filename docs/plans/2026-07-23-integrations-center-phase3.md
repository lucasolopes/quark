# Central de integracoes Fase 3 (connector_id + health) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Desambiguar Zapier/Make/n8n via `connector_id`, dar health passivo a webhooks e pixels (fora do hot path de clique), e tornar o dedup do Slack a prova de rename via `channel_id`.

**Architecture:** Aumentar in-place os tipos ja persistidos (`WebhookSubscription`, `PixelConfig`) em vez de introduzir uma tabela `Connection` generica. Um unico enum `HealthStatus` (Never/Ok/Error), espelhando o `SyncStatus` do Sheets, cobre webhook e pixel. Gravacao de health e best-effort e nunca ocorre no evento `link.clicked`.

**Tech Stack:** Rust (axum + tokio, async_trait, sqlx/Postgres, heed/LMDB, serde), React + TypeScript + Vite (Vitest).

## Global Constraints

- `cargo` nao esta no PATH: no Bash usar `export PATH="$HOME/.cargo/bin:$PATH"` antes de qualquer comando cargo.
- Sempre buildar/testar com `-j1` (ou `CARGO_BUILD_JOBS=1`).
- Gate por task: `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, testes lib (`cargo test --lib`), e testes gated de Postgres com `QUARK_TEST_DATABASE_URL` apontando para um usuario NAO-superuser.
- Sem `CREATE INDEX CONCURRENTLY`. Nenhuma tabela nova (o `TRUNCATE` de `reset_for_tests` fica intocado).
- `src/codec.rs` e `src/permute.rs` NAO podem ser tocados.
- Migracoes Postgres sao sempre `ALTER TABLE ... ADD COLUMN IF NOT EXISTS` (idempotentes), no bloco de schema de `postgres.rs`.
- Prosa (docs/commits) em pt-BR natural, sem em-dashes, seguindo `avoid-ai-writing`.
- Nao ha dados historicos a migrar (prod sera resetado): campos novos sao `#[serde(default)]` / colunas nullable, sem backfill.

---

### Task 1: Tipo compartilhado `HealthStatus`

Um enum unico reutilizado por webhook e pixel, com o mesmo shape de wire do `SyncStatus` do Sheets (`{ "state": "never" | "ok" | "error", "detail"?: string }`), default `Never`.

**Files:**
- Create: `src/health.rs`
- Modify: `src/lib.rs` (adicionar `pub mod health;` junto dos outros `pub mod`)
- Test: inline `#[cfg(test)]` em `src/health.rs`

**Interfaces:**
- Produces: `pub enum HealthStatus { Never, Ok, Error(String) }` com `Default = Never`, `Serialize`/`Deserialize` no shape `{state, detail}`; usado nas Tasks 2, 3, 4, 5, 6, 7, 9.

- [ ] **Step 1: Escrever o teste que falha**

Em `src/health.rs`:

```rust
//! Status compartilhado da ultima entrega/forward de uma integracao (LUC-87
//! fase 3). Espelha o shape do `sheets::SyncStatus` para que o front trate
//! health de webhook, pixel e sheets do mesmo jeito (`{ state, detail }`).

use serde::{Deserialize, Serialize};

/// Resultado da ultima entrega/forward de uma integracao. `Never` = conectado
/// mas sem entrega ainda; `Ok` = ultima entrega teve sucesso; `Error` carrega
/// um motivo curto (nunca um segredo/token).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", content = "detail", rename_all = "lowercase")]
pub enum HealthStatus {
    #[default]
    Never,
    Ok,
    Error(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_never() {
        assert_eq!(HealthStatus::default(), HealthStatus::Never);
    }

    #[test]
    fn serializes_with_state_tag_and_detail_content() {
        assert_eq!(
            serde_json::to_string(&HealthStatus::Never).unwrap(),
            r#"{"state":"never"}"#
        );
        assert_eq!(
            serde_json::to_string(&HealthStatus::Ok).unwrap(),
            r#"{"state":"ok"}"#
        );
        assert_eq!(
            serde_json::to_string(&HealthStatus::Error("boom".into())).unwrap(),
            r#"{"state":"error","detail":"boom"}"#
        );
    }

    #[test]
    fn round_trips() {
        for s in [
            HealthStatus::Never,
            HealthStatus::Ok,
            HealthStatus::Error("timeout".into()),
        ] {
            let j = serde_json::to_string(&s).unwrap();
            let back: HealthStatus = serde_json::from_str(&j).unwrap();
            assert_eq!(s, back);
        }
    }
}
```

- [ ] **Step 2: Rodar o teste e ver falhar**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -j1 --lib health::`
Expected: FAIL de compilacao (`module health not found`) ate o `pub mod health;` existir.

- [ ] **Step 3: Registrar o modulo**

Em `src/lib.rs`, adicionar na lista de modulos publicos (ordem alfabetica junto dos vizinhos, ex. perto de `pub mod guard;`/`pub mod import;`):

```rust
pub mod health;
```

- [ ] **Step 4: Rodar o teste e ver passar**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -j1 --lib health::`
Expected: PASS (3 testes).

- [ ] **Step 5: Commit**

```bash
git add src/health.rs src/lib.rs
git commit -m "feat(health): tipo HealthStatus compartilhado (LUC-87 fase 3)"
```

---

### Task 2: Campos novos em `WebhookSubscription`

Adiciona `connector_id`, `external_id`, `last_delivery_at`, `last_delivery_status` ao tipo, todos `#[serde(default)]` para que blobs LMDB antigos e linhas Postgres pre-existentes desserializem sem os campos.

**Files:**
- Modify: `src/webhooks/mod.rs:119-137` (struct `WebhookSubscription`)
- Modify (compiler-driven, adicionar os 4 campos a cada literal): `src/api/slack.rs`, `src/api/webhooks_api.rs`, `src/api/tests.rs`, `src/webhooks/delivery.rs` (helpers de teste), `src/domain_router.rs`, `tests/webhooks_store_it.rs`, `tests/webhook_outbox_it.rs`, `tests/tenant_isolation.rs`, `tests/tenant_enforcement.rs`, `tests/postgres_store_it.rs`
- Test: inline em `src/webhooks/mod.rs`

**Interfaces:**
- Consumes: `HealthStatus` (Task 1).
- Produces: `WebhookSubscription` com os campos `connector_id: Option<String>`, `external_id: Option<String>`, `last_delivery_at: Option<u64>`, `last_delivery_status: HealthStatus`.

- [ ] **Step 1: Escrever o teste que falha**

Inline em `src/webhooks/mod.rs` (dentro do `#[cfg(test)] mod tests`, ou criar um):

```rust
#[test]
fn legacy_json_without_phase3_fields_deserializes_with_defaults() {
    // Um blob gravado antes da fase 3 (sem connector_id/external_id/health).
    let legacy = r#"{"id":7,"url":"https://h/x","events":["link.created"],
        "secret":"","active":true,"created":100,"kind":"generic"}"#;
    let sub: WebhookSubscription = serde_json::from_str(legacy).unwrap();
    assert_eq!(sub.connector_id, None);
    assert_eq!(sub.external_id, None);
    assert_eq!(sub.last_delivery_at, None);
    assert_eq!(sub.last_delivery_status, crate::health::HealthStatus::Never);
}
```

- [ ] **Step 2: Rodar o teste e ver falhar**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -j1 --lib webhooks::`
Expected: FAIL de compilacao (`no field connector_id`).

- [ ] **Step 3: Adicionar os campos ao struct**

Em `src/webhooks/mod.rs`, no struct `WebhookSubscription` (apos `label`):

```rust
    /// Id do conector do catalogo (`"zapier"`, `"make"`, `"n8n"`, `"slack"`...).
    /// Desambigua os webhooks genericos, que compartilham `kind: Generic`.
    /// `None` em linhas anteriores a fase 3.
    #[serde(default)]
    pub connector_id: Option<String>,
    /// Id estavel do destino do lado do provedor (o Slack usa o `channel_id`),
    /// para dedup a prova de rename. Generico de proposito para reuso futuro.
    #[serde(default)]
    pub external_id: Option<String>,
    /// Timestamp (epoch secs) da ultima tentativa de entrega registrada.
    #[serde(default)]
    pub last_delivery_at: Option<u64>,
    /// Resultado da ultima entrega registrada (health passivo).
    #[serde(default)]
    pub last_delivery_status: crate::health::HealthStatus,
```

- [ ] **Step 4: Corrigir todos os literais (compiler-driven)**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo build -j1 --all-targets 2>&1 | grep -A3 "missing field"`

Para cada `WebhookSubscription { ... }` que o compilador apontar, adicionar as linhas:

```rust
    connector_id: None,
    external_id: None,
    last_delivery_at: None,
    last_delivery_status: Default::default(),
```

Excecao: na `src/api/slack.rs` (o `sub` construido no `slack_callback`, ~linha 161) usar `connector_id: Some("slack".to_string())` em vez de `None` (o Slack sempre conhece seu conector). `external_id` continua `None` aqui; a Task 8 o preenche.

- [ ] **Step 5: Rodar o teste e ver passar**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -j1 --lib webhooks::`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(webhooks): connector_id + health passivo em WebhookSubscription (LUC-87 fase 3)"
```

---

### Task 3: Campos novos em `PixelConfig`

Pixels ja se desambiguam por `provider`, entao NAO ganham `connector_id`. So health: `last_forward_at` + `last_forward_status`.

**Files:**
- Modify: `src/pixel.rs:47-54` (struct `PixelConfig`)
- Modify (compiler-driven): `src/pixel.rs` (helpers `ga4_config`/`meta_config` nos testes), `tests/pixel_store_it.rs`, `tests/pixel_forward_it.rs`, e qualquer outro literal apontado
- Test: inline em `src/pixel.rs`

**Interfaces:**
- Consumes: `HealthStatus` (Task 1).
- Produces: `PixelConfig` com `last_forward_at: Option<u64>`, `last_forward_status: HealthStatus`.

- [ ] **Step 1: Escrever o teste que falha**

Inline em `src/pixel.rs` (dentro de `mod tests`):

```rust
#[test]
fn legacy_pixel_json_without_health_deserializes_with_defaults() {
    let legacy = r#"{"id":1,"provider":"ga4",
        "credentials":{"measurement_id":"G-X","api_secret":"s"},
        "active":true,"created":10}"#;
    let cfg: PixelConfig = serde_json::from_str(legacy).unwrap();
    assert_eq!(cfg.last_forward_at, None);
    assert_eq!(cfg.last_forward_status, crate::health::HealthStatus::Never);
}
```

- [ ] **Step 2: Rodar o teste e ver falhar**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -j1 --lib pixel::`
Expected: FAIL de compilacao (`no field last_forward_at`).

- [ ] **Step 3: Adicionar os campos ao struct**

Em `src/pixel.rs`, no struct `PixelConfig` (apos `created`):

```rust
    /// Timestamp (epoch secs) do ultimo forward registrado (health passivo).
    #[serde(default)]
    pub last_forward_at: Option<u64>,
    /// Resultado do ultimo forward registrado.
    #[serde(default)]
    pub last_forward_status: crate::health::HealthStatus,
```

- [ ] **Step 4: Corrigir os literais (compiler-driven)**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo build -j1 --all-targets 2>&1 | grep -A3 "missing field"`

Para cada `PixelConfig { ... }`, adicionar:

```rust
    last_forward_at: None,
    last_forward_status: Default::default(),
```

- [ ] **Step 5: Rodar o teste e ver passar**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -j1 --lib pixel::`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(pixel): health passivo (last_forward) em PixelConfig (LUC-87 fase 3)"
```

---

### Task 4: `record_webhook_health` no trait `Store` (+ colunas e mapeamento)

Metodo de update cirurgico do health de um webhook, mais as colunas Postgres novas e a leitura/escrita delas.

**Files:**
- Modify: `src/store/mod.rs` (trait `Store`: declarar `record_webhook_health`)
- Modify: `src/store/lmdb.rs` (impl read-modify-write)
- Modify: `src/store/postgres.rs` (DDL `ALTER TABLE webhooks ADD COLUMN`; `row_to_webhook`; SELECTs de `list_webhooks`/`get_webhook`; `put_webhook`; impl `record_webhook_health`)
- Modify (compiler-driven): todo `impl Store for` de teste (`src/webhooks/delivery.rs` StubStore, `src/api/tests.rs`, `tests/common/mod.rs` se houver, etc.)
- Test: inline em `src/store/lmdb.rs`; gated em `tests/webhooks_store_it.rs`

**Interfaces:**
- Consumes: `HealthStatus` (Task 1), campos da Task 2.
- Produces: `async fn record_webhook_health(&self, tenant: TenantId, id: u64, at: u64, status: HealthStatus) -> Result<(), StoreError>`.

- [ ] **Step 1: Escrever o teste que falha (LMDB, inline)**

Em `src/store/lmdb.rs` (`#[cfg(test)] mod tests`), espelhando o padrao de `put_link_health`:

```rust
#[tokio::test]
async fn record_webhook_health_updates_only_health_fields() {
    let s = new_test_store();
    let sub = WebhookSubscription {
        id: 1, url: "https://h/x".into(),
        events: vec![crate::webhooks::EventType::LinkCreated],
        secret: String::new(), active: true, created: 10,
        kind: crate::webhooks::SubscriptionKind::Generic,
        label: None, connector_id: Some("zapier".into()),
        external_id: None, last_delivery_at: None,
        last_delivery_status: crate::health::HealthStatus::Never,
    };
    s.put_webhook(crate::tenant::DEFAULT_TENANT, &sub).await.unwrap();

    s.record_webhook_health(
        crate::tenant::DEFAULT_TENANT, 1, 200,
        crate::health::HealthStatus::Error("502".into()),
    ).await.unwrap();

    let got = s.get_webhook(crate::tenant::DEFAULT_TENANT, 1).await.unwrap().unwrap();
    assert_eq!(got.last_delivery_at, Some(200));
    assert_eq!(got.last_delivery_status, crate::health::HealthStatus::Error("502".into()));
    // Campos nao-health preservados.
    assert_eq!(got.connector_id.as_deref(), Some("zapier"));
    assert_eq!(got.url, "https://h/x");
    assert!(got.active);
}
```

(Se nao existir helper `new_test_store()`, usar o construtor de store de teste ja usado pelos testes vizinhos no arquivo, ex. o mesmo usado em `record_link_health`/`put_link_health` tests.)

- [ ] **Step 2: Rodar e ver falhar**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -j1 --lib store::lmdb::tests::record_webhook_health`
Expected: FAIL (`no method record_webhook_health`).

- [ ] **Step 3: Declarar no trait**

Em `src/store/mod.rs`, junto dos metodos de webhook (perto de `put_webhook`/`next_webhook_id`):

```rust
    /// Registra o resultado da ultima entrega de um webhook (health passivo,
    /// LUC-87 fase 3). Update cirurgico: nao reescreve os outros campos da
    /// subscription. Best-effort no caller (erro logado, nunca propagado ao
    /// hot path). No-op silencioso se a subscription nao existe mais.
    async fn record_webhook_health(
        &self,
        tenant: TenantId,
        id: u64,
        at: u64,
        status: crate::health::HealthStatus,
    ) -> Result<(), StoreError>;
```

- [ ] **Step 4: Impl no LMDB**

Em `src/store/lmdb.rs`, perto de `put_webhook`:

```rust
    async fn record_webhook_health(
        &self,
        tenant: crate::tenant::TenantId,
        id: u64,
        at: u64,
        status: crate::health::HealthStatus,
    ) -> Result<(), StoreError> {
        let mut wtxn = self.env.write_txn()?;
        let key = tkey_id(tenant, id);
        if let Some(bytes) = self.webhooks.get(&wtxn, &key)? {
            let mut sub: WebhookSubscription = serde_json::from_slice(bytes)?;
            sub.last_delivery_at = Some(at);
            sub.last_delivery_status = status;
            let out = serde_json::to_vec(&sub)?;
            self.webhooks.put(&mut wtxn, &key, &out)?;
            wtxn.commit()?;
        }
        Ok(())
    }
```

(Confirmar na implementacao se `self.webhooks.get` devolve `&[u8]` dentro da txn; ajustar o borrow/clone conforme o padrao dos outros metodos LMDB do arquivo, que ja fazem read-modify-write.)

- [ ] **Step 5: Impl no Postgres (colunas + mapeamento + update)**

Em `src/store/postgres.rs`:

5a. No bloco de schema (apos o `ALTER TABLE webhooks ADD COLUMN IF NOT EXISTS label TEXT`, ~linha 694):

```rust
                // Health passivo por webhook (LUC-87 fase 3): connector_id
                // desambigua os genericos; external_id casa o destino do lado
                // do provedor; last_delivery_* guardam a ultima entrega.
                "ALTER TABLE webhooks ADD COLUMN IF NOT EXISTS connector_id TEXT",
                "ALTER TABLE webhooks ADD COLUMN IF NOT EXISTS external_id TEXT",
                "ALTER TABLE webhooks ADD COLUMN IF NOT EXISTS last_delivery_at BIGINT",
                "ALTER TABLE webhooks ADD COLUMN IF NOT EXISTS last_delivery_status JSONB",
```

5b. Em `row_to_webhook` (linha 367), antes do `Ok(WebhookSubscription {`:

```rust
    let connector_id: Option<String> = r.try_get("connector_id").map_err(StoreError::backend)?;
    let external_id: Option<String> = r.try_get("external_id").map_err(StoreError::backend)?;
    let last_delivery_at: Option<i64> = r.try_get("last_delivery_at").map_err(StoreError::backend)?;
    let last_delivery_status: Option<serde_json::Value> =
        r.try_get("last_delivery_status").map_err(StoreError::backend)?;
```

e no corpo do struct:

```rust
        connector_id,
        external_id,
        last_delivery_at: last_delivery_at.map(|v| v as u64),
        last_delivery_status: match last_delivery_status {
            Some(v) => serde_json::from_value(v)?,
            None => crate::health::HealthStatus::Never,
        },
```

5c. Nos SELECTs de `list_webhooks` (linha 1402) e `get_webhook` (1418), trocar a lista de colunas para:

```
SELECT id, url, events, secret, active, created, kind, label, connector_id, external_id, last_delivery_at, last_delivery_status FROM webhooks ...
```

5d. Em `put_webhook` (linha 1438), incluir as colunas novas (mantem o padrao numerado):

```rust
            sqlx::query(
                "INSERT INTO webhooks (id, url, events, secret, active, created, kind, tenant_id, label, connector_id, external_id, last_delivery_at, last_delivery_status) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) \
                 ON CONFLICT (id) DO UPDATE SET url=$2, events=$3, secret=$4, active=$5, created=$6, kind=$7, tenant_id=$8, label=$9, connector_id=$10, external_id=$11, last_delivery_at=$12, last_delivery_status=$13",
            )
            .bind(sub.id as i64)
            .bind(&sub.url)
            .bind(&events)
            .bind(&sub.secret)
            .bind(sub.active)
            .bind(sub.created as i64)
            .bind(sub.kind.as_str())
            .bind(tenant.0 as i64)
            .bind(&sub.label)
            .bind(&sub.connector_id)
            .bind(&sub.external_id)
            .bind(sub.last_delivery_at.map(|v| v as i64))
            .bind(serde_json::to_value(&sub.last_delivery_status)?)
            .execute(&mut *c)
            .await
```

5e. Impl do metodo (update cirurgico), perto de `put_webhook`:

```rust
    async fn record_webhook_health(
        &self,
        tenant: TenantId,
        id: u64,
        at: u64,
        status: crate::health::HealthStatus,
    ) -> Result<(), StoreError> {
        let status = serde_json::to_value(&status)?;
        with_write!(self, tenant, |c| {
            sqlx::query(
                "UPDATE webhooks SET last_delivery_at=$1, last_delivery_status=$2 WHERE tenant_id=$3 AND id=$4",
            )
            .bind(at as i64)
            .bind(&status)
            .bind(tenant.0 as i64)
            .bind(id as i64)
            .execute(&mut *c)
            .await
        });
        Ok(())
    }
```

- [ ] **Step 6: Impl nos stubs de teste (compiler-driven)**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo build -j1 --all-targets 2>&1 | grep -B1 "not all trait items"`

Para cada `impl Store for` de teste que faltar o metodo, adicionar. No `StubStore` de `src/webhooks/delivery.rs` a impl precisa CAPTURAR a chamada (a Task 6 testa a gravacao), entao usar um campo compartilhado em vez de `unimplemented!()`:

```rust
    async fn record_webhook_health(
        &self,
        tenant: crate::tenant::TenantId,
        id: u64,
        at: u64,
        status: crate::health::HealthStatus,
    ) -> Result<(), StoreError> {
        self.health_calls.lock().unwrap().push((tenant, id, at, status));
        Ok(())
    }
```

Adicionar o campo `health_calls: std::sync::Mutex<Vec<(crate::tenant::TenantId, u64, u64, crate::health::HealthStatus)>>` ao `StubStore` e inicializar como `Mutex::new(Vec::new())` nos construtores `new`/`new_multi`. Nos demais stubs (que nao exercitam health) usar `Ok(())` simples.

- [ ] **Step 7: Rodar os testes e ver passar**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -j1 --lib store::lmdb::tests::record_webhook_health`
Expected: PASS.

Gated (se o Postgres de teste estiver disponivel):
Run: `export PATH="$HOME/.cargo/bin:$PATH" && QUARK_TEST_DATABASE_URL=postgres://quark_test@localhost/quark_test cargo test -j1 --test webhooks_store_it`
Expected: PASS (adicionar, em `tests/webhooks_store_it.rs`, um teste gated que grava e le o health round-trip via `put_webhook`/`record_webhook_health`/`get_webhook`, espelhando o teste LMDB do Step 1).

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(store): record_webhook_health + colunas de connector_id/health (LUC-87 fase 3)"
```

---

### Task 5: `record_pixel_health` no trait `Store` (+ colunas e mapeamento)

Analogo a Task 4, para pixels.

**Files:**
- Modify: `src/store/mod.rs` (trait), `src/store/lmdb.rs` (impl), `src/store/postgres.rs` (DDL `pixels`; `row_to_pixel`; SELECTs de `list_pixels`/`get_pixel`; `put_pixel`; impl)
- Modify (compiler-driven): stubs `impl Store for`
- Test: inline em `src/store/lmdb.rs`; gated em `tests/pixel_store_it.rs`

**Interfaces:**
- Consumes: `HealthStatus` (Task 1), campos da Task 3.
- Produces: `async fn record_pixel_health(&self, tenant: TenantId, id: u64, at: u64, status: HealthStatus) -> Result<(), StoreError>`.

- [ ] **Step 1: Escrever o teste que falha (LMDB, inline)**

Em `src/store/lmdb.rs`:

```rust
#[tokio::test]
async fn record_pixel_health_updates_only_health_fields() {
    let s = new_test_store();
    let cfg = crate::pixel::PixelConfig {
        id: 3, provider: crate::pixel::Provider::Ga4,
        credentials: crate::pixel::PixelCredentials {
            measurement_id: Some("G-X".into()), api_secret: Some("s".into()),
            pixel_id: None, access_token: None,
        },
        active: true, created: 10,
        last_forward_at: None,
        last_forward_status: crate::health::HealthStatus::Never,
    };
    s.put_pixel(crate::tenant::DEFAULT_TENANT, &cfg).await.unwrap();

    s.record_pixel_health(
        crate::tenant::DEFAULT_TENANT, 3, 300,
        crate::health::HealthStatus::Ok,
    ).await.unwrap();

    let got = s.get_pixel(crate::tenant::DEFAULT_TENANT, 3).await.unwrap().unwrap();
    assert_eq!(got.last_forward_at, Some(300));
    assert_eq!(got.last_forward_status, crate::health::HealthStatus::Ok);
    assert_eq!(got.credentials.measurement_id.as_deref(), Some("G-X"));
}
```

- [ ] **Step 2: Rodar e ver falhar**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -j1 --lib store::lmdb::tests::record_pixel_health`
Expected: FAIL (`no method record_pixel_health`).

- [ ] **Step 3: Declarar no trait**

Em `src/store/mod.rs`, junto dos metodos de pixel:

```rust
    /// Registra o resultado do ultimo forward de um pixel (health passivo,
    /// LUC-87 fase 3). Update cirurgico; best-effort no caller; no-op se o
    /// pixel nao existe mais.
    async fn record_pixel_health(
        &self,
        tenant: TenantId,
        id: u64,
        at: u64,
        status: crate::health::HealthStatus,
    ) -> Result<(), StoreError>;
```

- [ ] **Step 4: Impl no LMDB**

Em `src/store/lmdb.rs`, perto de `put_pixel`:

```rust
    async fn record_pixel_health(
        &self,
        tenant: crate::tenant::TenantId,
        id: u64,
        at: u64,
        status: crate::health::HealthStatus,
    ) -> Result<(), StoreError> {
        let mut wtxn = self.env.write_txn()?;
        let key = tkey_id(tenant, id);
        if let Some(bytes) = self.pixels.get(&wtxn, &key)? {
            let mut cfg: crate::pixel::PixelConfig = serde_json::from_slice(bytes)?;
            cfg.last_forward_at = Some(at);
            cfg.last_forward_status = status;
            let out = serde_json::to_vec(&cfg)?;
            self.pixels.put(&mut wtxn, &key, &out)?;
            wtxn.commit()?;
        }
        Ok(())
    }
```

- [ ] **Step 5: Impl no Postgres (colunas + mapeamento + update)**

5a. No bloco de schema (apos o `CREATE TABLE IF NOT EXISTS pixels`, ~linha 705):

```rust
                // Health passivo por pixel (LUC-87 fase 3).
                "ALTER TABLE pixels ADD COLUMN IF NOT EXISTS last_forward_at BIGINT",
                "ALTER TABLE pixels ADD COLUMN IF NOT EXISTS last_forward_status JSONB",
```

5b. Em `row_to_pixel` (linha 485), ler as colunas novas e preencher o struct:

```rust
    let last_forward_at: Option<i64> = r.try_get("last_forward_at").map_err(StoreError::backend)?;
    let last_forward_status: Option<serde_json::Value> =
        r.try_get("last_forward_status").map_err(StoreError::backend)?;
```

e no corpo:

```rust
        last_forward_at: last_forward_at.map(|v| v as u64),
        last_forward_status: match last_forward_status {
            Some(v) => serde_json::from_value(v)?,
            None => crate::health::HealthStatus::Never,
        },
```

5c. Nos SELECTs de `list_pixels`/`get_pixel` (localizar as strings `SELECT ... FROM pixels`), incluir `last_forward_at, last_forward_status`.

5d. Em `put_pixel` (linha 2034), incluir as colunas novas no INSERT/ON CONFLICT (mesmo padrao numerado da Task 4, `$7` e `$8`), fazendo bind de `config.last_forward_at.map(|v| v as i64)` e `serde_json::to_value(&config.last_forward_status)?`.

5e. Impl do metodo:

```rust
    async fn record_pixel_health(
        &self,
        tenant: TenantId,
        id: u64,
        at: u64,
        status: crate::health::HealthStatus,
    ) -> Result<(), StoreError> {
        let status = serde_json::to_value(&status)?;
        with_write!(self, tenant, |c| {
            sqlx::query(
                "UPDATE pixels SET last_forward_at=$1, last_forward_status=$2 WHERE tenant_id=$3 AND id=$4",
            )
            .bind(at as i64)
            .bind(&status)
            .bind(tenant.0 as i64)
            .bind(id as i64)
            .execute(&mut *c)
            .await
        });
        Ok(())
    }
```

- [ ] **Step 6: Impl nos stubs (compiler-driven)**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo build -j1 --all-targets 2>&1 | grep -B1 "not all trait items"`

No `StubStore` de `src/analytics/mod.rs` (se houver, para a Task 7) capturar as chamadas num `Mutex<Vec<...>>` como na Task 4; nos demais, `Ok(())`.

- [ ] **Step 7: Rodar os testes e ver passar**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -j1 --lib store::lmdb::tests::record_pixel_health`
Expected: PASS.

Gated: adicionar teste round-trip em `tests/pixel_store_it.rs` e rodar com `QUARK_TEST_DATABASE_URL`.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(store): record_pixel_health + colunas de health de pixel (LUC-87 fase 3)"
```

---

### Task 6: Gravar health de webhook na entrega (fora do hot path)

Thread `&store` + tenant ate `deliver_one`; grava health apos o POST, exceto para `EventType::LinkClicked`. O relay Postgres (`deliver_claimed`) grava tambem.

**Files:**
- Modify: `src/webhooks/delivery.rs` (`spawn_webhook_worker`, `deliver_to_matching`, `deliver_to_matching_guarded`, `deliver_one`, `deliver_claimed`)
- Test: inline em `src/webhooks/delivery.rs` (usando o `StubStore` com `health_calls` da Task 4)

**Interfaces:**
- Consumes: `record_webhook_health` (Task 4), `HealthStatus` (Task 1).
- Produces: comportamento de gravacao de health; nada novo consumido por outras tasks.

- [ ] **Step 1: Escrever os testes que falham**

Em `src/webhooks/delivery.rs` (`mod tests`), dois testes usando o servidor local e o `StubStore`:

```rust
#[tokio::test]
async fn records_health_ok_for_non_clicked_event() {
    let (url, _state) = spawn_test_server(vec![200]).await;
    let sub = test_sub(1, &url, crate::webhooks::SubscriptionKind::Generic,
                       vec![EventType::LinkCreated]);
    let store: Arc<dyn Store> = Arc::new(StubStore::new(vec![sub.clone()]));
    let subs = vec![(crate::tenant::DEFAULT_TENANT, vec![sub])];
    let ev = test_event(EventType::LinkCreated, crate::tenant::DEFAULT_TENANT);

    deliver_to_matching_guarded(&reqwest::Client::new(), &store, &subs, &ev, |_| false).await;

    let calls = store_health_calls(&store);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1, 1); // id
    assert_eq!(calls[0].3, crate::health::HealthStatus::Ok);
}

#[tokio::test]
async fn does_not_record_health_for_link_clicked() {
    let (url, _state) = spawn_test_server(vec![200]).await;
    let sub = test_sub(2, &url, crate::webhooks::SubscriptionKind::Generic,
                       vec![EventType::LinkClicked]);
    let store: Arc<dyn Store> = Arc::new(StubStore::new(vec![sub.clone()]));
    let subs = vec![(crate::tenant::DEFAULT_TENANT, vec![sub])];
    let ev = test_event(EventType::LinkClicked, crate::tenant::DEFAULT_TENANT);

    deliver_to_matching_guarded(&reqwest::Client::new(), &store, &subs, &ev, |_| false).await;

    assert!(store_health_calls(&store).is_empty(),
        "link.clicked nunca deve gravar health (hot path)");
}
```

Adicionar os helpers de teste que faltarem: `test_sub(id, url, kind, events)` (constroi `WebhookSubscription` com os campos da Task 2), `test_event(event_type, tenant)` (constroi `WebhookEvent` com `body` JSON valido), e `store_health_calls(&Arc<dyn Store>)` que faz downcast do `StubStore` e clona `health_calls`. Se o downcast for inconveniente, expor um `Arc<StubStore>` concreto no teste e passar `store.clone() as Arc<dyn Store>` mantendo o `Arc<StubStore>` para inspecao.

- [ ] **Step 2: Rodar e ver falhar**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -j1 --lib webhooks::delivery::tests::records_health`
Expected: FAIL de compilacao (`deliver_to_matching_guarded` ainda nao recebe `&store`).

- [ ] **Step 3: Thread do store e gravacao no in-memory worker**

Em `src/webhooks/delivery.rs`:

3a. `spawn_webhook_worker`: na chamada `deliver_to_matching(&client, &subs, &ev)` (linha 206), passar o store: `deliver_to_matching(&client, &store, &subs, &ev).await`.

3b. `deliver_to_matching` e `deliver_to_matching_guarded`: adicionar `store: &Arc<dyn Store>` na assinatura (logo apos `client`), e repassar a `deliver_one`.

3c. `deliver_one`: nova assinatura `async fn deliver_one(client, store: &Arc<dyn Store>, sub, ev)`. Capturar o resultado em vez de `return` cedo:

```rust
async fn deliver_one(
    client: &reqwest::Client,
    store: &Arc<dyn Store>,
    sub: &WebhookSubscription,
    ev: &WebhookEvent,
) {
    let Some(req) = build_outgoing_request(sub, ev, None) else {
        return;
    };
    let mut outcome = crate::health::HealthStatus::Error("no attempt".into());
    for attempt in 0..DELIVERY_ATTEMPTS {
        // ...loop existente, mas em vez de `return` no sucesso:
        match res {
            Ok(resp) if resp.status().is_success() => {
                outcome = crate::health::HealthStatus::Ok;
                break;
            }
            Ok(resp) => {
                outcome = crate::health::HealthStatus::Error(format!("status {}", resp.status().as_u16()));
                // ...log existente...
            }
            Err(e) => {
                outcome = crate::health::HealthStatus::Error(e.to_string());
                // ...log existente...
            }
        }
        // ...backoff existente...
    }
    // ...log de exhausted existente (mantido)...

    // Health passivo: nunca no hot path de clique. Best-effort.
    if ev.event_type != EventType::LinkClicked {
        if let Err(e) = store.record_webhook_health(ev.tenant_id, sub.id, crate::now(), outcome).await {
            eprintln!("{}", serde_json::json!({"webhook_health_record_error": e.to_string()}));
        }
    }
}
```

- [ ] **Step 4: Gravacao no relay Postgres**

Em `deliver_claimed` (linha 569): apos `store.mark_delivered` (sucesso) gravar `HealthStatus::Ok`; no caminho de falha (antes de `mark_retry`/`mark_dead`) gravar `HealthStatus::Error(...)`. Sempre com o guard de clique:

```rust
    if !matches!(event_type, EventType::LinkClicked) {
        let _ = store
            .record_webhook_health(delivery.tenant_id, sub.id, now, outcome)
            .await;
    }
```

(Definir `outcome` a partir do resultado do `post_once`: `Ok` no ramo de sucesso, `Error("delivery failed")` no ramo de falha. `event_type` ja esta disponivel no escopo.)

- [ ] **Step 5: Corrigir outras chamadas de `deliver_one`/`deliver_to_matching` (compiler-driven)**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo build -j1 --all-targets`
Corrigir quaisquer chamadas restantes (ex. testes que chamavam `deliver_one` com a assinatura antiga) passando um `store` stub.

- [ ] **Step 6: Rodar os testes e ver passar**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -j1 --lib webhooks::delivery::`
Expected: PASS (incluindo os dois novos + os pre-existentes).

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(webhooks): grava health passivo na entrega, exceto no hot path de clique (LUC-87 fase 3)"
```

---

### Task 7: Gravar health de pixel no forward (worker de analytics)

Grava o resultado do ultimo forward por pixel config, dentro do `flush` do worker de analytics. Best-effort; ja roda fora do hot path de redirect.

**Files:**
- Modify: `src/analytics/mod.rs` (funcao `flush`, laco de forward ~linha 740-748; precisa do `store` no escopo do `flush`)
- Test: inline em `src/analytics/mod.rs` (StubStore capturando `record_pixel_health`) ou gated em `tests/pixel_forward_it.rs`

**Interfaces:**
- Consumes: `record_pixel_health` (Task 5), `HealthStatus` (Task 1).
- Produces: comportamento de gravacao; nada novo para outras tasks.

- [ ] **Step 1: Escrever o teste que falha**

Preferir um teste gated em `tests/pixel_forward_it.rs` que, apos um `flush` com um mock server que responde 200, verifica via `get_pixel` que `last_forward_status == Ok` e `last_forward_at.is_some()`; e com o mock respondendo 500, verifica `Error`. Se `flush` nao for facilmente chamavel de fora, adicionar um teste inline no `mod tests` de `analytics/mod.rs` usando um `StubStore` que capture `record_pixel_health`.

```rust
// Esboco (ajustar a assinatura real de `flush`):
#[tokio::test]
async fn flush_records_pixel_health_ok_on_success() {
    // mock server 200, um PixelConfig ativo, um ClickEvent no buffer
    // apos flush: assert record_pixel_health chamado com (tenant, id, _, Ok)
}
```

- [ ] **Step 2: Rodar e ver falhar**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -j1 --lib analytics::`
Expected: FAIL (health nao gravado ainda) ou falha de compilacao se o teste referencia algo novo.

- [ ] **Step 3: Passar o `store` ao `flush` e gravar**

3a. Garantir que `flush` recebe `store: &Arc<dyn Store>` (o worker em `spawn_worker` ja tem `store` no escopo; adicionar o parametro na assinatura de `flush` e nas suas chamadas em `spawn_worker`, incluindo a do ramo do ticker linha 640 e a do ramo de evento).

3b. No laco de forward (linha 740), capturar o resultado e gravar por config:

```rust
        for config in configs.iter().filter(|c| c.active) {
            let base = bases.base_for(config.provider);
            let status = match pixel::forward(client, base, config, &scoped, key, bases.anonymize_ip).await {
                Ok(()) => crate::health::HealthStatus::Ok,
                Err(e) => {
                    eprintln!(/* ...log existente... */);
                    crate::health::HealthStatus::Error(e.to_string())
                }
            };
            let _ = store.record_pixel_health(*tenant, config.id, crate::now(), status).await;
        }
```

(`tenant` e `config` ja estao no escopo do laco `for (tenant, configs)`; confirmar o nome exato da variavel de tenant no `flush`.)

- [ ] **Step 4: Corrigir chamadas de `flush` (compiler-driven)**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo build -j1 --all-targets`
Passar `&store` em todas as chamadas de `flush`.

- [ ] **Step 5: Rodar os testes e ver passar**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -j1 --lib analytics::`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(analytics): grava health passivo de pixel no forward (LUC-87 fase 3)"
```

---

### Task 8: Slack, dedup a prova de rename por `channel_id`

Extrai o `channel_id` da resposta do OAuth e o usa como `external_id` no dedup do callback.

**Files:**
- Modify: `src/slack.rs` (struct `IncomingWebhook`: `channel_id`)
- Modify: `src/api/slack.rs` (`slack_callback`: grava `external_id`; dedup por `external_id` -> `label` -> `url`)
- Test: inline em `src/slack.rs` (parse); e um teste de dedup (inline em `src/api/slack.rs` se houver helpers, ou gated se precisar de store real)

**Interfaces:**
- Consumes: campos `external_id`/`connector_id` da Task 2.
- Produces: `IncomingWebhook.channel_id: Option<String>`.

- [ ] **Step 1: Escrever o teste que falha (parse do channel_id)**

Em `src/slack.rs` (`mod tests`):

```rust
#[test]
fn parses_channel_id_from_incoming_webhook() {
    let json = r##"{"ok":true,"incoming_webhook":{"url":"https://hooks.slack.com/services/T/B/x","channel":"#general","channel_id":"C012AB3CD","configuration_url":"https://team.slack.com/services/B"}}"##;
    let parsed: OAuthAccess = serde_json::from_str(json).unwrap();
    let wh = parsed.incoming_webhook.unwrap();
    assert_eq!(wh.channel_id.as_deref(), Some("C012AB3CD"));
}
```

- [ ] **Step 2: Rodar e ver falhar**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -j1 --lib slack::tests::parses_channel_id`
Expected: FAIL (`no field channel_id`).

- [ ] **Step 3: Adicionar o campo**

Em `src/slack.rs`, no struct `IncomingWebhook` (apos `channel`):

```rust
    #[serde(default)]
    pub channel_id: Option<String>,
```

- [ ] **Step 4: Usar no callback**

Em `src/api/slack.rs` (`slack_callback`, ~linha 111-152):

4a. Apos extrair `webhook`, capturar `let channel_id = webhook.channel_id.filter(|c| !c.is_empty());`.

4b. No dedup (`existing.iter().find(...)`, linha 131), preferir `external_id`:

```rust
    let dup = existing.iter().find(|s| {
        s.kind == SubscriptionKind::Slack
            && ((channel_id.is_some() && s.external_id == channel_id)
                || (channel_id.is_none() && label_ref.is_some() && s.label.as_deref() == label_ref)
                || s.url == url)
    });
```

4c. No update in-place (linha 136), reafirmar `external_id` e `connector_id`:

```rust
        let updated = WebhookSubscription {
            url: url.clone(),
            label: label.clone().or_else(|| dup.label.clone()),
            external_id: channel_id.clone().or_else(|| dup.external_id.clone()),
            connector_id: Some("slack".to_string()),
            ..dup.clone()
        };
```

4d. Na insercao nova (`sub`, linha 161), setar `external_id: channel_id.clone()` (e `connector_id: Some("slack".to_string())` ja veio da Task 2).

- [ ] **Step 5: Teste de dedup a prova de rename**

Adicionar um teste (inline se os helpers de store de teste estiverem acessiveis a `src/api/slack.rs`; caso contrario gated em `tests/` com `TestState`). O teste: insere uma subscription Slack com `external_id = Some("C012")` e `label = Some("#general")`; simula um re-install com o mesmo `channel_id` mas `channel = "#renamed"`; verifica que apos o callback existe UMA subscription (nao duas) e que `label` virou `#renamed` e `url` foi atualizada. Se um teste end-to-end do callback for pesado, no minimo testar a funcao de dedup extraida (considerar extrair o predicado de match para uma `fn` pura testavel `slack_dup_index(existing, kind, channel_id, label, url)`).

- [ ] **Step 6: Rodar os testes e ver passar**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -j1 --lib slack:: && cargo test -j1 --lib api::slack`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(slack): dedup a prova de rename por channel_id (external_id) (LUC-87 fase 3)"
```

---

### Task 9: Superficie de API (connector_id + health nas rows)

Expoe `connector_id` no create e nas rows de webhook; expoe health em `WebhookRow`/`PixelRow`.

**Files:**
- Modify: `src/api/webhooks_api.rs` (`WebhookCreateReq`, `WebhookRow`, `admin_webhooks_list`, `admin_webhooks_create`; `PixelRow`, `to_pixel_row`)
- Test: `tests/webhooks_api_it.rs` (ou `src/api/tests.rs`)

**Interfaces:**
- Consumes: campos das Tasks 2, 3.
- Produces: JSON de `GET /admin/webhooks` com `connector_id`, `last_delivery_at`, `last_delivery_status`; `GET /admin/pixels` com `last_forward_at`, `last_forward_status`; `POST /admin/webhooks` aceitando `connector_id`.

- [ ] **Step 1: Escrever o teste que falha**

Em `tests/webhooks_api_it.rs`, um teste que cria um webhook com `connector_id: "zapier"` e verifica que o `GET` retorna esse `connector_id` e `last_delivery_status.state == "never"`:

```rust
#[tokio::test]
async fn create_and_list_webhook_exposes_connector_id_and_health() {
    let st = TestState::new().build();
    // POST /admin/webhooks com body {"url":..,"events":["link.created"],"kind":"generic","connector_id":"zapier"}
    // GET /admin/webhooks -> row.connector_id == "zapier", row.last_delivery_status == {"state":"never"}
}
```

- [ ] **Step 2: Rodar e ver falhar**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -j1 --test webhooks_api_it create_and_list_webhook_exposes`
Expected: FAIL (campos ausentes).

- [ ] **Step 3: Estender os tipos de request/response**

3a. `WebhookCreateReq` (linha 4): adicionar `#[serde(default)] connector_id: Option<String>`.

3b. `WebhookRow` (linha 20): adicionar `connector_id: Option<String>` (com `#[serde(skip_serializing_if = "Option::is_none")]`), `last_delivery_at: Option<u64>`, `last_delivery_status: crate::health::HealthStatus`.

3c. `admin_webhooks_list` (linha 43): preencher os campos novos a partir de `s`.

3d. `admin_webhooks_create`: setar `connector_id: req.connector_id` na `WebhookSubscription` criada (e `external_id: None`, `last_delivery_at: None`, `last_delivery_status: Default::default()`).

3e. `PixelRow` (linha 114): adicionar `last_forward_at: Option<u64>`, `last_forward_status: crate::health::HealthStatus`; `to_pixel_row` (linha 123) preenche a partir de `config`.

- [ ] **Step 4: Rodar os testes e ver passar**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -j1 --test webhooks_api_it`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(api): expoe connector_id e health nas rows de webhook/pixel (LUC-87 fase 3)"
```

---

### Task 10: Frontend, tipos + render de health + filtro por connector_id

**Files:**
- Modify: `web/src/lib/types.ts` (`Webhook`, `Pixel`, novo tipo `HealthStatus`; `CreateWebhookRequest`)
- Modify: `web/src/lib/connectors.ts` (`useConnectedIds` por `connector_id`)
- Modify: `web/src/routes/ExtensionDetail.tsx` (`WebhookPanel` filtra por `connector_id` e mostra health; `PixelPanel` mostra health)
- Modify: `web/src/lib/queries.ts` / api client (passar `connector_id` no create, se aplicavel)
- Test: `web/src/**/*.test.tsx` (Vitest)

**Interfaces:**
- Consumes: JSON da Task 9.
- Produces: UI final.

- [ ] **Step 1: Escrever o teste que falha (Vitest)**

Em `web/src/lib/connectors.test.tsx` (ou arquivo vizinho), um teste que monta `useConnectedIds` com dois webhooks genericos de `connector_id` distintos (`"zapier"` e `"make"`) e verifica que so os ids corretos acendem (nao os tres juntos). E um teste de render do `WebhookPanel`/`PixelPanel` mostrando "ultima entrega" quando `last_delivery_status.state === "ok"`.

- [ ] **Step 2: Rodar e ver falhar**

Run (no diretorio `web/`): `npm test -- connectors`
Expected: FAIL.

- [ ] **Step 3: Estender os tipos**

Em `web/src/lib/types.ts`:

```ts
/** Status da ultima entrega/forward de uma integracao (espelha o backend). */
export interface HealthStatus { state: "never" | "ok" | "error"; detail?: string; }
```

- No `Webhook` (linha 166): `connector_id?: string | null;`, `last_delivery_at?: number | null;`, `last_delivery_status: HealthStatus;`.
- No `Pixel` (linha 212): `last_forward_at?: number | null;`, `last_forward_status: HealthStatus;`.
- No `CreateWebhookRequest` (linha 179): `connector_id?: string;`.

- [ ] **Step 4: `useConnectedIds` por connector_id**

Em `web/src/lib/connectors.ts` (`useConnectedIds`, linha 104): quando um webhook tem `connector_id`, casar por ele; senao (legacy) manter o casamento por `kind`. Atualizar o comentario de limitacao (linhas 96-103) para refletir que a ambiguidade dos genericos esta resolvida quando `connector_id` esta presente.

- [ ] **Step 5: Render de health nos paineis**

Em `web/src/routes/ExtensionDetail.tsx`:
- `WebhookPanel` (linha 239): `existing` passa a filtrar por `connector_id` quando presente (`w.connector_id === integration.id`), caindo para `kind` no legacy. Adicionar uma linha de health espelhando o `SheetsPanel` (linhas 201-206): "ultima entrega ha X" quando `state === "ok"`, texto de erro (`text-destructive`) quando `state === "error"`, nada quando `"never"`. Enviar `connector_id: integration.id` no `createWebhook.mutateAsync`.
- `PixelPanel` (linha 499): mesma linha de health a partir de `last_forward_status`/`last_forward_at`.
- Reusar `formatDateTime` (ja importado) para o timestamp.

Adicionar as chaves i18n necessarias (ex. `extensions.webhookLastDelivery`, `extensions.webhookDeliveryError`, `extensions.pixelLastForward`, `extensions.pixelForwardError`) nos arquivos de mensagens, seguindo o padrao das chaves `sheetsLastSync`/`sheetsSyncError`.

- [ ] **Step 6: Rodar os testes e ver passar**

Run (em `web/`): `npm test`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(web): render de health e filtro por connector_id nos paineis (LUC-87 fase 3)"
```

---

### Task 11: Documentacao

**Files:**
- Modify/Create: `docs/WEBHOOKS.md` + `docs/WEBHOOKS.PT_BR.md` (se cobrirem health/connector_id) ou uma nota na doc da central de integracoes
- Test: n/a (revisao de prosa via `avoid-ai-writing`)

- [ ] **Step 1: Atualizar a doc**

Documentar, a nivel humano, o health passivo (o que "ultima entrega" significa, por que `link.clicked` nao conta) e o `connector_id` (por que Zapier/Make/n8n agora se distinguem). Manter o par EN + PT_BR com o header de troca de idioma. Passar a prosa pela skill `avoid-ai-writing`.

- [ ] **Step 2: Commit**

```bash
git add -A
git commit -m "docs: health passivo e connector_id na central de integracoes (LUC-87 fase 3)"
```

---

## Self-Review

**Cobertura do spec:**
- connector_id nos webhooks genericos -> Tasks 2, 8 (slack), 9 (API), 10 (front). OK.
- Health passivo de webhook, fora do hot path -> Tasks 1, 4, 6. OK (guard `LinkClicked` explicito em 6).
- Health passivo de pixel -> Tasks 1, 5, 7. OK.
- Match do Slack por channel_id -> Task 8. OK.
- Superficie API + front -> Tasks 9, 10. OK.
- Persistencia (ALTER TABLE, LMDB serde default, sem tabela nova) -> Tasks 4, 5. OK.
- Constraints (codec/permute intocados, -j1, PG nao-superuser) -> Global Constraints + gates. OK.

**Placeholders:** os `grep` compiler-driven sao acoes concretas (nao "implemente depois"). Os testes de front (Task 10 Step 1) e de dedup (Task 8 Step 5) tem esboco em vez de codigo completo porque dependem de helpers do repo (TestState, render util) que o implementador confirma na hora; todos os testes de backend tem codigo completo. Sinalizado como ponto de atencao.

**Consistencia de tipos:** `HealthStatus` (um unico tipo) usado em `last_delivery_status` e `last_forward_status`; assinatura `record_webhook_health`/`record_pixel_health(tenant, id, at, status)` identica entre trait, lmdb, postgres e stubs; `external_id`/`connector_id` com os mesmos nomes do struct (Task 2) ao uso no Slack (Task 8) e API (Task 9). OK.
