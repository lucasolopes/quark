# LUC-34 — Sheets sync por tenant Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Fazer o sync do Google Sheets operar por tenant: `sheets::sync` respeita um tenant, o handler on-demand passa o tenant do principal, e o worker agendado itera todos os tenants com conexão.

**Architecture:** `sheets::sync` ganha um parâmetro `tenant: TenantId` usado em `list_links`/`visits`. O worker agendado em `main.rs` passa a iterar `list_tenants()` (padrão do LUC-36). Sem mudança no trait `Store`.

**Tech Stack:** Rust, tokio, axum.

## Global Constraints

- Sem novo método no trait `Store`. Iterar via `list_tenants()` + `get_sheets_connection(t)`.
- OSS/single-tenant deve degradar ao comportamento atual (um tenant no loop).
- Erro de um tenant no worker agendado não aborta a varredura dos demais.
- Base URL das short URLs continua o `QUARK_PUBLIC_HOST` global (per-tenant domain é follow-up, fora de escopo).
- Tipos: `TenantId(pub u64)`, `DEFAULT_TENANT = TenantId(0)`, `Tenant { id: TenantId, name, slug, created }`. `store.list_links(TenantId, Option<u64>, usize, Option<&str>, Option<&str>)`, `store.visits(TenantId, u64)`, `store.list_tenants() -> Vec<Tenant>`.
- Comentários em inglês técnico direto, sem em-dash.

---

### Task 1: Teste de isolamento de `sheets::sync` por tenant (RED)

**Files:**
- Create: `tests/sheets_sync_it.rs`

**Interfaces:**
- Consumes: `quark::sheets::{sync, SheetsConnection, SyncStatus}`, `quark::sheets::client::SheetsApi`, `quark::store::open_backends`, `quark::tenant::{Tenant, TenantId, DEFAULT_TENANT}`, `quark::store::Record`.
- Produces: nada (teste).

- [ ] **Step 1: Escrever o teste com um mock SheetsApi (RED)**

`sync` ainda não tem parâmetro `tenant`, então este teste NÃO COMPILA até a Task 2 — é o gancho RED (falha de compilação primeiro, depois asserção).

```rust
use async_trait::async_trait;
use quark::sheets::client::SheetsApi;
use quark::sheets::{sync, SheetsConnection, SyncStatus};
use quark::store::{open_backends, Record};
use quark::tenant::{Tenant, TenantId, DEFAULT_TENANT};
use std::sync::{Arc, Mutex};

struct MockApi {
    rows: Arc<Mutex<Vec<Vec<String>>>>,
}

#[async_trait]
impl SheetsApi for MockApi {
    async fn create_spreadsheet(&self, _tok: &str, _title: &str) -> Result<String, String> {
        Ok("sheet-1".to_string())
    }
    async fn update_values(
        &self,
        _tok: &str,
        _sid: &str,
        rows: &[Vec<String>],
    ) -> Result<(), String> {
        *self.rows.lock().unwrap() = rows.to_vec();
        Ok(())
    }
}

fn rec(url: &str, tenant: TenantId) -> Record {
    Record {
        url: url.into(),
        expiry: None,
        created: 1_700_000_000,
        tags: vec![],
        max_visits: None,
        rules: vec![],
        variants: vec![],
        app_ios: None,
        app_android: None,
        folder: None,
        fallback_url: None,
        password_hash: None,
        tenant_id: tenant,
    }
}

#[tokio::test]
async fn sync_reads_catalog_of_the_given_tenant_only() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _sink) = open_backends(dir.path(), true).await.unwrap();

    let tenant_b = Tenant {
        id: TenantId(1),
        name: "Tenant B".into(),
        slug: "tenant-b".into(),
        created: 0,
    };
    store.put_tenant(&tenant_b).await.unwrap();

    // Tenant 0 (default) tem um link; tenant 1 tem outro.
    store
        .put_link(DEFAULT_TENANT, 1, &rec("https://tenant0.example", DEFAULT_TENANT))
        .await
        .unwrap();
    store
        .put_link(TenantId(1), 2, &rec("https://tenant1.example", TenantId(1)))
        .await
        .unwrap();

    let rows = Arc::new(Mutex::new(Vec::new()));
    let api = MockApi { rows: rows.clone() };
    let mut conn = SheetsConnection {
        refresh_token: "rt".into(),
        email: "b@example.com".into(),
        spreadsheet_id: None,
        last_sync: None,
        last_status: SyncStatus::Never,
    };

    sync(
        &store,
        &api,
        0x1234,
        "https://s.example",
        &mut conn,
        "access-token",
        1_752_300_000,
        TenantId(1),
    )
    .await
    .unwrap();

    let written = rows.lock().unwrap();
    let flat: String = written.iter().flatten().cloned().collect::<Vec<_>>().join("|");
    assert!(
        flat.contains("https://tenant1.example"),
        "deve conter o link do tenant 1: {flat}"
    );
    assert!(
        !flat.contains("https://tenant0.example"),
        "NÃO deve conter o link do tenant 0 (vazamento): {flat}"
    );
    assert_eq!(conn.last_status, SyncStatus::Ok);
}
```

- [ ] **Step 2: Rodar e confirmar que FALHA (compilação)**

Run (PowerShell; cargo não está no PATH — usar caminho completo):
`& "$env:USERPROFILE\.cargo\bin\cargo.exe" test --test sheets_sync_it 2>&1 | Select-Object -Last 20`
Expected: erro de compilação — `sync` recebe 8 args mas o teste passa 8 incluindo `TenantId(1)`; hoje `sync` tem 7 parâmetros (sem `tenant`). Erro tipo "this function takes 7 arguments but 8 were supplied".

---

### Task 2: `sheets::sync` recebe e respeita `tenant` (GREEN parte 1)

**Files:**
- Modify: `src/sheets/mod.rs` (assinatura de `sync`, `list_links`, `visits`)

**Interfaces:**
- Consumes: `crate::tenant::TenantId`.
- Produces: `sync(..., tenant: TenantId)` consumido pelo teste (Task 1) e pelos call sites (Task 3).

- [ ] **Step 1: Adicionar o parâmetro `tenant` a `sync`**

Em `src/sheets/mod.rs`, na assinatura de `pub async fn sync(...)`, adicionar como ÚLTIMO parâmetro (após `now: u64,`):
```rust
    now: u64,
    tenant: crate::tenant::TenantId,
) -> Result<(), String> {
```

- [ ] **Step 2: Usar `tenant` no `list_links` e `visits`**

Trocar (dentro de `sync`):
- `store.list_links(crate::tenant::DEFAULT_TENANT, after, SYNC_PAGE, None, None)` → `store.list_links(tenant, after, SYNC_PAGE, None, None)`
- `store.visits(crate::tenant::DEFAULT_TENANT, *id)` → `store.visits(tenant, *id)`

- [ ] **Step 3: Rodar o teste da Task 1 e confirmar GREEN**

Run: `& "$env:USERPROFILE\.cargo\bin\cargo.exe" test --test sheets_sync_it 2>&1 | Select-Object -Last 20`
Expected: PASS (`sync_reads_catalog_of_the_given_tenant_only`).

Nota: os call sites em `api.rs` e `main.rs` ainda não passam `tenant` → `cargo build` do binário vai falhar. É o gancho da Task 3. (O teste de integração compila porque exercita só a lib.)

---

### Task 3: Call sites passam o tenant (GREEN parte 2)

**Files:**
- Modify: `src/api.rs` (handler `sheets_sync`, ~L3533)
- Modify: `src/main.rs` (worker agendado, ~L473-528)

**Interfaces:**
- Consumes: `sync(..., tenant)` (Task 2), `store.list_tenants()`, `store.get_sheets_connection(TenantId)`, `store.put_sheets_connection(TenantId, &conn)`.

- [ ] **Step 1: On-demand handler passa `p.tenant`**

Em `src/api.rs`, na chamada `crate::sheets::sync(...)` dentro de `sheets_sync` (a que passa `&mut conn`), adicionar o argumento final `p.tenant`:
```rust
                crate::sheets::sync(
                    &st.store,
                    api.as_ref(),
                    st.key,
                    &base_url,
                    &mut conn,
                    &access_token,
                    now(),
                    p.tenant,
                )
                .await
```

- [ ] **Step 2: Worker agendado itera tenants**

Em `src/main.rs`, dentro do `tokio::spawn` do sync agendado, substituir o corpo do loop APÓS a aquisição da lease (o bloco que hoje faz `get_sheets_connection(DEFAULT_TENANT)` ... `put_sheets_connection(DEFAULT_TENANT)`) por uma iteração de tenants:

```rust
                    let tenants = match store.list_tenants().await {
                        Ok(t) => t,
                        Err(e) => {
                            eprintln!(
                                "{}",
                                serde_json::json!({ "sheets_sync_list_tenants_error": e.to_string() })
                            );
                            continue;
                        }
                    };
                    for t in tenants {
                        let Ok(Some(mut conn)) = store.get_sheets_connection(t.id).await else {
                            continue;
                        };
                        let outcome = match quark::sheets::refresh_access_token(
                            &client,
                            &cfg,
                            &conn.refresh_token,
                        )
                        .await
                        {
                            Ok(token) => {
                                quark::sheets::sync(
                                    &store,
                                    api.as_ref(),
                                    key,
                                    &base_url,
                                    &mut conn,
                                    &token,
                                    quark::now(),
                                    t.id,
                                )
                                .await
                            }
                            Err(e) => Err(e),
                        };
                        if let Err(e) = &outcome {
                            conn.last_status = quark::sheets::SyncStatus::Error(e.clone());
                            eprintln!(
                                "{}",
                                serde_json::json!({ "sheets_sync_error": e, "tenant": t.id.0 })
                            );
                        } else {
                            eprintln!(
                                "{}",
                                serde_json::json!({ "sheets_sync": "ok", "tenant": t.id.0 })
                            );
                        }
                        if let Err(e) = store.put_sheets_connection(t.id, &conn).await {
                            eprintln!(
                                "{}",
                                serde_json::json!({ "sheets_sync_persist_error": e.to_string(), "tenant": t.id.0 })
                            );
                        }
                    }
```

(Mantém a aquisição da lease `try_acquire_sheets_lease` e o `continue` antes desse bloco exatamente como estão. Só o miolo pós-lease muda.)

- [ ] **Step 3: Build + suíte + fmt/clippy**

Run:
- `& "$env:USERPROFILE\.cargo\bin\cargo.exe" build`
- `& "$env:USERPROFILE\.cargo\bin\cargo.exe" test --test sheets_sync_it`
- `& "$env:USERPROFILE\.cargo\bin\cargo.exe" test --lib sheets`
- `& "$env:USERPROFILE\.cargo\bin\cargo.exe" test` (suíte completa)
- `& "$env:USERPROFILE\.cargo\bin\cargo.exe" fmt`
- `& "$env:USERPROFILE\.cargo\bin\cargo.exe" clippy --all-targets -- -D warnings`
Expected: tudo verde, sem warnings.

- [ ] **Step 4: Commit**

```bash
git add src/sheets/mod.rs src/api.rs src/main.rs tests/sheets_sync_it.rs
git commit -m "feat(sheets): sync por tenant (on-demand + agendado iteram tenants) (LUC-34)"
```

---

### Task 4: Documentação (doc-it + avoid-ai-writing)

**Files:**
- Modify: `docs/SHEETS.md`, `docs/SHEETS.PT_BR.md` (se afirmarem single-tenant/uma conexão global)

- [ ] **Step 1:** Invocar `doc-it` escopado ao Sheets: achar afirmações de que existe "uma conexão global"/single-tenant que ficaram imprecisas; em cloud cada tenant conecta sua própria conta Google e o sync é por tenant. Em OSS continua uma conexão (um operador). Edição cirúrgica.
- [ ] **Step 2:** Rodar `avoid-ai-writing` edit-in-place nos arquivos tocados; manter EN/PT_BR sincronizados e o cabeçalho de troca de idioma.
- [ ] **Step 3:** Commit:
```bash
git add docs/
git commit -m "docs: sync do Sheets agora é por tenant (LUC-34)"
```

---

## Self-Review

**1. Spec coverage:** `sync` recebe tenant → Task 2; on-demand passa tenant → Task 3 Step 1; worker itera tenants → Task 3 Step 2; erro por tenant não aborta → Task 3 Step 2 (loga e segue); OSS inalterado → list_tenants degenera; teste isolamento → Task 1; docs → Task 4. ✓
**2. Placeholder scan:** sem placeholders; todo código presente. ✓
**3. Type consistency:** `tenant: TenantId` (último param) usado igual em Task 1 (chamada de teste), Task 2 (assinatura), Task 3 (dois call sites). `t.id` é `TenantId`, `t.id.0` é `u64` (só em log). ✓
