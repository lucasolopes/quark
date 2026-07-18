# LUC-36 — Forward de pixels por tenant Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fazer o worker de forward de pixels carregar e encaminhar conversões por tenant, fechando o vazamento cross-tenant, sem tocar no trait `Store` nem nas assinaturas públicas do worker.

**Architecture:** O snapshot em memória do worker passa de `Vec<PixelConfig>` para `Vec<(TenantId, Vec<PixelConfig>)>`. `refresh_pixel_snapshot` enumera todos os tenants via `list_tenants()` + `list_pixels(t)`; `forward_to_pixels` filtra o batch por `tenant_id` uma vez por tenant antes de encaminhar. Fail-open preservado nos dois lados.

**Tech Stack:** Rust, tokio, axum (mock server nos testes de integração), clickhouse (não tocado aqui).

## Global Constraints

- Toda a mudança de produção fica em `src/analytics/mod.rs`. Sem mudança no trait `Store` (`src/store/mod.rs`), em `spawn_worker`/`flush`/`pixel::forward`.
- Fail-open: erro/timeout de store mantém o snapshot anterior; erro de provider só é logado (nunca propagado).
- OSS/single-tenant (`list_tenants()` devolve só o default) deve degradar para o comportamento atual, byte-for-byte.
- Tipos: `TenantId(pub u64)` em `src/tenant.rs`; `DEFAULT_TENANT: TenantId = TenantId(0)`; `Tenant { id: TenantId, .. }`; `PixelConfig { id: u64, provider, credentials, active: bool, created: u64 }` (sem `tenant_id`); `ClickEvent { .. , tenant_id: u64 }`.
- Prosa/comentários seguem avoid-ai-writing (inglês técnico direto, sem em-dash), consistente com o resto de `mod.rs`.
- Documentação da fase de implementação: rodar as skills `doc-it` e `avoid-ai-writing` ao final (Task 4).

---

### Task 1: Teste de isolamento cross-tenant (RED)

Prova o comportamento alvo antes de qualquer mudança de produção: dois tenants, cada um com um pixel apontando para um mock distinto, um batch misturado, cada mock recebe só os eventos do seu tenant. Este teste FALHA hoje (o snapshot só carrega tenant 0, então o mock do tenant 1 não recebe nada).

**Files:**
- Modify: `tests/pixel_forward_it.rs` (adicionar helper `ev_t` + novo teste)

**Interfaces:**
- Consumes: `spawn_worker(rx, sink, store, client, key, bases)`, `open_backends(dir, multi_tenant: bool)`, `PixelBases`, `PixelConfig`, `store.put_pixel(TenantId, &cfg)`, `store.put_tenant(&Tenant)`, `quark::tenant::{Tenant, TenantId, DEFAULT_TENANT}`.
- Produces: nada (arquivo de teste).

Fatos confirmados na investigação:
- `Tenant { id: TenantId, name: String, slug: String, created: u64 }` — NÃO deriva `Default`, construir todos os campos.
- `store.put_tenant(&self, t: &Tenant) -> Result<(), StoreError>` (trait em `src/store/mod.rs:536`).
- LMDB semeia `DEFAULT_TENANT` no boot; `list_tenants` devolve todos os tenants persistidos.
- Um worker tem um `PixelBases` só (um base GA4). Então os dois pixels batem no MESMO mock; o isolamento é provado por contagem de chamadas + `link_code` por chamada.

- [ ] **Step 1: Adicionar helper `ev_t` e o teste (RED)**

Em `tests/pixel_forward_it.rs`, adicionar abaixo do helper `ev`:

```rust
fn ev_t(id: u64, ts: u64, tenant: u64) -> ClickEvent {
    ClickEvent {
        tenant_id: tenant,
        ..ev(id, ts)
    }
}
```

E o teste. Um mock, um worker; tenant A = `DEFAULT_TENANT` (0, já semeado), tenant B = 1 (criado). Cada tenant tem um pixel GA4 ativo. O batch tem um clique de cada. Isolamento => o mock recebe exatamente 2 chamadas (uma por pixel), e NENHUMA chamada carrega os dois `link_code` juntos (isso é o que aconteceria se o batch não fosse filtrado por tenant); a união dos codes das duas chamadas é `{code_a, code_b}`.

```rust
#[tokio::test]
async fn worker_forwards_only_matching_tenant_events_to_each_pixel() {
    let (mock_base, captured) = mock_server("/mp/collect").await;
    let dir = tempfile::tempdir().unwrap();
    let (store, sink) = open_backends(dir.path(), true).await.unwrap();

    // Tenant A = DEFAULT_TENANT (0), semeado no boot. Tenant B = 1, criado agora.
    let tenant_b = quark::tenant::Tenant {
        id: quark::tenant::TenantId(1),
        name: "Tenant B".into(),
        slug: "tenant-b".into(),
        created: 0,
    };
    store.put_tenant(&tenant_b).await.unwrap();

    store
        .put_pixel(quark::tenant::DEFAULT_TENANT, &ga4_config(1))
        .await
        .unwrap();
    store
        .put_pixel(quark::tenant::TenantId(1), &ga4_config(1))
        .await
        .unwrap();

    let bases = PixelBases {
        ga4: mock_base,
        meta: "http://127.0.0.1:1".to_string(),
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<ClickEvent>(100);
    let handle =
        spawn_worker(rx, sink.clone(), store.clone(), reqwest::Client::new(), KEY, bases);

    tx.send(ev_t(10, 1_752_300_000, 0)).await.unwrap(); // tenant A
    tx.send(ev_t(20, 1_752_300_001, 1)).await.unwrap(); // tenant B
    drop(tx);
    handle.await.unwrap();

    let code_a = codec::to_base62(permute::encode(10, KEY));
    let code_b = codec::to_base62(permute::encode(20, KEY));

    let calls = captured.lock().unwrap();
    // Uma chamada por pixel (dois pixels), não uma só com o batch inteiro.
    assert_eq!(calls.len(), 2, "cada pixel encaminha uma vez");

    // Os link_codes de CADA chamada. Isolamento => cada chamada carrega só o
    // code do seu tenant; vazamento => alguma chamada carrega os dois.
    for (_, body) in calls.iter() {
        let events = body["events"].as_array().unwrap();
        let codes: Vec<&str> = events
            .iter()
            .map(|e| e["params"]["link_code"].as_str().unwrap())
            .collect();
        assert_eq!(codes.len(), 1, "cada chamada tem só o clique do seu tenant");
        assert!(
            codes[0] == code_a || codes[0] == code_b,
            "code inesperado: {:?}",
            codes[0]
        );
    }

    // Uma chamada com code_a (tenant A) e outra com code_b (tenant B).
    let seen: Vec<&str> = calls
        .iter()
        .map(|(_, b)| b["events"][0]["params"]["link_code"].as_str().unwrap())
        .collect();
    assert!(seen.contains(&code_a.as_str()), "tenant A não encaminhado");
    assert!(seen.contains(&code_b.as_str()), "tenant B não encaminhado");
}
```

- [ ] **Step 2: Rodar o teste e confirmar que FALHA (RED)**

Run: `cargo test --test pixel_forward_it worker_forwards_only_matching_tenant_events_to_each_pixel -- --nocapture`
Expected: FALHA. Hoje o snapshot só carrega `list_pixels(DEFAULT_TENANT)`: só o pixel do tenant A existe no snapshot e recebe o batch inteiro (os dois cliques). `calls.len()` será 1, e essa chamada carregará os DOIS codes — falha no `assert_eq!(calls.len(), 2)` e no `assert_eq!(codes.len(), 1)`.

---

### Task 2: Snapshot por tenant + refresh iterando tenants (GREEN, parte 1)

**Files:**
- Modify: `src/analytics/mod.rs` (assinatura/tipo do snapshot, `refresh_pixel_snapshot`)

**Interfaces:**
- Consumes: `store.list_tenants() -> Result<Vec<Tenant>, StoreError>`, `store.list_pixels(TenantId) -> Result<Vec<PixelConfig>, StoreError>`, `crate::tenant::TenantId`.
- Produces: tipo de snapshot `Vec<(crate::tenant::TenantId, Vec<PixelConfig>)>` consumido por `flush`/`forward_to_pixels` (Task 3).

- [ ] **Step 1: Importar `TenantId` no topo do módulo**

Em `src/analytics/mod.rs:1-2`, ajustar o import do store para incluir o tipo de tenant. Adicionar após a linha `use crate::store::{Store, StoreError};`:

```rust
use crate::tenant::TenantId;
```

- [ ] **Step 2: Trocar o tipo do snapshot em `spawn_worker`**

Em `spawn_worker` (`~L373`), trocar:

```rust
        let mut pixels: Vec<PixelConfig> = Vec::new();
```

por:

```rust
        // Snapshot de pixels agrupado por tenant. Cada clique é encaminhado só
        // aos pixels do seu próprio tenant (isolamento cross-tenant), então o
        // tenant dono precisa viajar junto: `PixelConfig` não carrega tenant.
        let mut pixels: Vec<(TenantId, Vec<PixelConfig>)> = Vec::new();
```

- [ ] **Step 3: Reescrever `refresh_pixel_snapshot` para iterar tenants**

Substituir a função inteira (`~L408-426`) por:

```rust
/// Refreshes the cached pixel-config snapshot from `store`, across every
/// tenant, bounded by `PIXEL_SNAPSHOT_TIMEOUT`. Fail-open: on a store error
/// (listing tenants or any tenant's pixels) or a timeout, the previous
/// snapshot (`pixels`) is left untouched and the failure is only logged, so a
/// wedged or erroring store never stalls the worker and never empties out a
/// snapshot that was previously known-good.
///
/// In OSS/single-tenant mode `list_tenants` returns only the default tenant,
/// so this degrades to exactly the old single-tenant behavior.
async fn refresh_pixel_snapshot(
    store: &Arc<dyn Store>,
    pixels: &mut Vec<(TenantId, Vec<PixelConfig>)>,
) {
    let load = async {
        let tenants = store.list_tenants().await?;
        let mut out: Vec<(TenantId, Vec<PixelConfig>)> = Vec::with_capacity(tenants.len());
        for t in tenants {
            let configs = store.list_pixels(t.id).await?;
            if !configs.is_empty() {
                out.push((t.id, configs));
            }
        }
        Ok::<_, StoreError>(out)
    };
    match tokio::time::timeout(PIXEL_SNAPSHOT_TIMEOUT, load).await {
        Ok(Ok(snapshot)) => *pixels = snapshot,
        Ok(Err(e)) => {
            eprintln!("{}", serde_json::json!({"pixel_list_error": e.to_string()}));
        }
        Err(_) => {
            eprintln!(
                "{}",
                serde_json::json!({"pixel_list_error": "timed out refreshing pixel snapshot"})
            );
        }
    }
}
```

- [ ] **Step 4: Atualizar o doc-comment de `PIXEL_SNAPSHOT_TIMEOUT`**

Em `~L340`, trocar a referência stale `store.list_pixels(crate::tenant::DEFAULT_TENANT)` por texto que reflita a enumeração por tenant:

```rust
/// How long a full pixel-snapshot refresh (`list_tenants` + `list_pixels` per
/// tenant) is allowed to run before it's abandoned in favor of the previous
/// snapshot (fail-open: a wedged store must never stall the worker).
```

- [ ] **Step 5: Compilar (espera-se erro só em `flush`/`forward_to_pixels`)**

Run: `cargo build`
Expected: erro de tipo em `flush`/`forward_to_pixels` (ainda esperam `&[PixelConfig]`). É o gancho pro Task 3. Se compilar sem erro, algo está errado — pare e investigue.

---

### Task 3: `forward_to_pixels` filtra por tenant (GREEN, parte 2)

**Files:**
- Modify: `src/analytics/mod.rs` (`flush`, `forward_to_pixels`)

**Interfaces:**
- Consumes: snapshot `&[(TenantId, Vec<PixelConfig>)]` (Task 2), `bases.base_for(provider)`, `pixel::forward(client, base, config, events, key)`.
- Produces: comportamento final (isolamento) que o teste do Task 1 verifica.

- [ ] **Step 1: Atualizar a assinatura de `flush`**

Em `flush` (`~L428-435`), trocar o parâmetro:

```rust
    pixels: &[PixelConfig],
```

por:

```rust
    pixels: &[(TenantId, Vec<PixelConfig>)],
```

(As chamadas a `flush` em `spawn_worker` passam `&pixels`, que agora já é do tipo novo — sem mudança nos call sites.)

- [ ] **Step 2: Reescrever `forward_to_pixels`**

Substituir a função inteira (`~L454-473`) por:

```rust
/// Forwards the flushed batch to every active pixel config in the cached
/// `pixels` snapshot (no store access on this path, see `spawn_worker`).
///
/// Tenant isolation: the batch mixes clicks from every tenant, so for each
/// tenant we forward only that tenant's own events to that tenant's pixels.
/// A tenant's conversion data never reaches another tenant's provider.
///
/// Fail-open: a per-provider forward failure is only logged, never propagated
/// (never affects the sink write above nor the redirect hot path, which has
/// already returned by the time this runs).
async fn forward_to_pixels(
    pixels: &[(TenantId, Vec<PixelConfig>)],
    client: &reqwest::Client,
    key: u64,
    bases: &PixelBases,
    events: &[ClickEvent],
) {
    for (tenant, configs) in pixels {
        if !configs.iter().any(|c| c.active) {
            continue;
        }
        let scoped: Vec<ClickEvent> = events
            .iter()
            .filter(|e| e.tenant_id == tenant.0)
            .cloned()
            .collect();
        if scoped.is_empty() {
            continue;
        }
        for config in configs.iter().filter(|c| c.active) {
            let base = bases.base_for(config.provider);
            if let Err(e) = pixel::forward(client, base, config, &scoped, key).await {
                eprintln!(
                    "{}",
                    serde_json::json!({
                        "pixel_forward_error": e.to_string(),
                        "pixel_id": config.id,
                    })
                );
            }
        }
    }
}
```

- [ ] **Step 3: Rodar o teste novo e confirmar GREEN**

Run: `cargo test --test pixel_forward_it worker_forwards_only_matching_tenant_events_to_each_pixel -- --nocapture`
Expected: PASS. Cada pixel recebe uma chamada com só o `link_code` do seu tenant.

- [ ] **Step 4: Rodar toda a suíte de forward + a inline de analytics (regressão)**

Run: `cargo test --test pixel_forward_it && cargo test --lib analytics`
Expected: PASS em todos os 6 testes de `pixel_forward_it` (5 antigos + o novo) e nos testes inline de `analytics`. Os 5 antigos exercitam tenant 0, o caso degenerado.

- [ ] **Step 5: `fmt` + `clippy` + commit**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings`
Expected: sem warnings.

```bash
git add src/analytics/mod.rs tests/pixel_forward_it.rs
git commit -m "feat(analytics): forward de pixels por tenant + isolamento cross-tenant (LUC-36)"
```

---

### Task 4: Documentação (doc-it + avoid-ai-writing)

**Files:**
- Modify: docs de analytics/extensões que mencionem forward de pixels/conversão (a descobrir via doc-it).

**Interfaces:**
- Consumes: nada de código.
- Produces: docs atualizadas refletindo que o forward de conversão é por tenant.

- [ ] **Step 1: Rodar a skill doc-it com foco na mudança**

Invocar a skill `doc-it` escopada ao forward de pixels/analytics: identificar docs (ex. `docs/` sobre pixels/extensões/analytics e seus `.PT_BR.md`) que afirmem ou impliquem que o forward é global/single-tenant, e atualizá-las para "por tenant".

- [ ] **Step 2: Rodar a skill avoid-ai-writing nas docs tocadas**

Invocar `avoid-ai-writing` em modo edit-in-place nos arquivos que o doc-it alterou, garantindo a prosa direta (sem em-dash) e o par EN/PT_BR consistente.

- [ ] **Step 3: Commit da documentação**

```bash
git add docs/
git commit -m "docs: forward de conversão agora é por tenant (LUC-36)"
```

---

## Self-Review

**1. Spec coverage:**
- Snapshot por tenant + refresh iterando `list_tenants()` → Task 2. ✓
- Forward filtra por `tenant_id` (fecha vazamento) → Task 3. ✓
- Fail-open (timeout único na enumeração, erro de provider logado) → Task 2 Step 3, Task 3 Step 2. ✓
- OSS/single-tenant inalterado → coberto pelos 5 testes antigos (Task 3 Step 4) + doc na função. ✓
- Teste de isolamento → Task 1. ✓
- Docs (doc-it + avoid-ai-writing) → Task 4. ✓
- ClickHouse/Postgres: já prontos, sem tarefa (confirmado na investigação). ✓

**2. Placeholder scan:** sem placeholders. API de `put_tenant`, struct `Tenant` (sem `Default`) e estratégia de harness (um worker/um mock, asserção por contagem+`link_code`) confirmadas na investigação e fixadas no Task 1.

**3. Type consistency:** `Vec<(TenantId, Vec<PixelConfig>)>` usado igual em `spawn_worker` (Task 2 Step 2), `refresh_pixel_snapshot` (Task 2 Step 3), `flush` (Task 3 Step 1), `forward_to_pixels` (Task 3 Step 2). `tenant.0` (u64) comparado com `e.tenant_id` (u64). ✓
