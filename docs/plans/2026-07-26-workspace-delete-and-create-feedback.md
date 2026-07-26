# LUC-138 + LUC-139 — Exclusão de workspace e feedback na criação (plano)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Um `Owner` consegue excluir seu workspace pelo painel, com todos os dados do tenant apagados nos três backends e o realm do Keycloak removido; e criar workspace deixa de parecer travado.

**Architecture:** `DELETE /admin/tenants/:id` autoriza por sessão e papel `Owner`, apaga os dados numa transação de Postgres (18 tabelas de `TENANT_OWNED_TABLES` mais `memberships` e `tenants`), dispara a exclusão no ClickHouse, e só depois do commit apaga o realm do Keycloak, best-effort com log. O painel expõe a ação atrás de confirmação por digitação do slug.

**Tech Stack:** Rust (axum 0.8, sqlx 0.9, heed 0.22, reqwest), React + TypeScript + TanStack Query, Vitest.

**Spec:** `docs/specs/2026-07-26-workspace-delete-and-create-feedback-design.md`

## Global Constraints

- Nada toca o hot path do redirect. `src/codec.rs` e `src/permute.rs` intocáveis.
- Nenhuma crate nem pacote npm novo.
- Comentários e chaves de log em inglês; prosa de doc em cada idioma, sem tradução literal e sem travessão.
- Níveis de log: `warn!` para todo caminho fail-open, `error!` para o que precisa de atenção, `info!` para lifecycle.
- Gate: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --lib`, e as binárias de Postgres. Sempre `-j1` / `CARGO_BUILD_JOBS=1`.
- **Postgres de teste:** `QUARK_TEST_DATABASE_URL="postgres://quark:quark@127.0.0.1:55432/quark"`, role NÃO-superusuária. O estado de RLS se acumula durante a rodada, então rode **cada binária isolada, recriando o banco antes de cada uma**:
  ```bash
  reset_db() {
    docker exec quark-test-pg psql -U postgres -c "DROP DATABASE IF EXISTS quark;" >/dev/null
    docker exec quark-test-pg psql -U postgres -c "CREATE DATABASE quark OWNER quark;" >/dev/null
  }
  ```
  `oidc_config_it` tem 4 falhas pré-existentes (LUC-143). Qualquer outra é sua.
- **NÃO use `git add -A`**: `docs/research/2026-07-24-shortio-benchmark/` está não-rastreado desde antes desta sessão.

---

### Task 1: `delete_realm` no cliente do Keycloak

**Files:**
- Modify: `src/keycloak/mod.rs` (trait `KeycloakAdmin`, linha 34)
- Modify: `src/keycloak/client.rs` (impl, ao lado dos `ensure_*`)
- Modify: os fakes de `KeycloakAdmin` em `tests/workspace_it.rs`

**Interfaces:**
- Produces:
  ```rust
  /// Apaga o realm do tenant. Um 404 (realm ja nao existe) e sucesso, pelo
  /// mesmo motivo que 409 e sucesso nos `ensure_*`: a operacao e idempotente.
  async fn delete_realm(&self, slug: &str) -> Result<(), KcError>;
  ```

- [ ] **Step 1: Escrever o teste que falha**

Em `tests/workspace_it.rs`, junto dos testes de provisionamento. O fake de Keycloak do arquivo já registra as chamadas; estenda-o para registrar `delete_realm` e escreva:

```rust
#[tokio::test]
async fn delete_realm_is_called_with_the_tenant_slug() {
    // Usa o mesmo fake dos testes de create_tenant_provisions_keycloak_realm.
    let kc = FakeKeycloak::default();
    kc.delete_realm("acme").await.unwrap();
    assert_eq!(kc.calls(), vec!["delete_realm:acme"]);
}
```

- [ ] **Step 2: Rodar e confirmar que falha**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
CARGO_BUILD_JOBS=1 cargo test -j1 --test workspace_it 2>&1 | tail -20
```
Esperado: `no method named delete_realm`.

- [ ] **Step 3: Implementar**

No trait, com o doc comment acima. Em `src/keycloak/client.rs`, reaproveitando o helper `admin_delete` que já existe (`client.rs:260`) e que já trata retry em 401/403:

```rust
async fn delete_realm(&self, slug: &str) -> Result<(), KcError> {
    let url = format!("{}/admin/realms/{slug}", self.base_url);
    self.admin_delete(&url).await
}
```

Confirme como os outros métodos montam a URL antes de copiar essa forma, e confirme que `admin_delete` trata `404` como sucesso; se não tratar, trate aqui, com comentário explicando que idempotência é o mesmo contrato dos `ensure_*`.

- [ ] **Step 4: Gate e commit**

```bash
CARGO_BUILD_JOBS=1 cargo test -j1 --test workspace_it 2>&1 | grep -E "^test result"
cargo fmt --check && CARGO_BUILD_JOBS=1 cargo clippy -j1 --all-targets -- -D warnings 2>&1 | tail -2
```
```bash
git add src/keycloak/mod.rs src/keycloak/client.rs tests/workspace_it.rs
git commit -m "feat(keycloak): delete_realm (LUC-138)

Excluir um workspace precisa remover o realm que a criacao provisiona. O
cliente so tinha os cinco ensure_*; um 404 conta como sucesso, pelo mesmo
motivo que 409 conta nos ensure_*: a operacao e idempotente.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: `delete_tenant_data` no `AnalyticsSink`

Separado da Task 3 porque é outro trait e outro backend.

**Files:**
- Modify: `src/analytics/mod.rs` (trait `AnalyticsSink`, linha 363)
- Modify: `src/analytics/clickhouse.rs`
- Modify: implementações do trait onde existirem (LMDB/Postgres via `Store`)

**Interfaces:**
- Produces:
  ```rust
  /// Apaga todo evento de clique do tenant. No ClickHouse isso e um
  /// `ALTER TABLE ... DELETE`, que e mutation ASSINCRONA: a chamada retorna
  /// quando a mutation foi aceita, nao quando terminou.
  async fn delete_tenant_data(&self, tenant: u64) -> Result<(), AnalyticsError>;
  ```

- [ ] **Step 1: Escrever o teste que falha**

Cubra o backend que der para exercitar sem ClickHouse rodando (o sink embarcado). O teste do ClickHouse fica gated pela env var como os demais desse arquivo; se não houver ClickHouse local, escreva-o mesmo assim e diga no relatório que não rodou.

```rust
#[tokio::test]
async fn delete_tenant_data_removes_only_that_tenants_clicks() {
    // grava cliques para o tenant 1 e o tenant 2,
    // chama delete_tenant_data(1),
    // confirma que stats_for_tenant(1) zerou e stats_for_tenant(2) nao.
}
```

O assert que importa é o **segundo**: apagar demais é o modo de falha perigoso aqui.

- [ ] **Step 2: Rodar, confirmar a falha, implementar**

No ClickHouse:
```rust
async fn delete_tenant_data(&self, tenant: u64) -> Result<(), AnalyticsError> {
    // Mutation assincrona: aceita agora, executada depois. O contrato com o
    // usuario esta documentado em docs/ como remocao eventual.
    self.execute("ALTER TABLE clicks DELETE WHERE tenant_id = ?", tenant).await
}
```
Adapte à forma real de executar query do arquivo (veja como `stats_for_tenant` monta e envia).

Para os backends que guardam clique no `Store`, a exclusão já acontece na Task 3 (a tabela `click_events` está em `TENANT_OWNED_TABLES`), então a implementação pode ser um no-op documentado. **Documente por que é no-op**, senão parece esquecimento.

- [ ] **Step 3: Gate e commit**

---

### Task 3: `delete_tenant` no `Store`

O coração da exclusão.

**Files:**
- Modify: `src/store/mod.rs` (trait `Store`, junto de `put_tenant` na linha 628)
- Modify: `src/store/postgres.rs`
- Modify: `src/store/lmdb.rs`
- Modify: mocks de `Store` em `src/domain_router.rs` e `src/webhooks/delivery.rs`
- Test: `tests/workspace_it.rs`, `tests/tenant_isolation_it.rs`

**Interfaces:**
- Produces:
  ```rust
  /// Apaga o tenant e TODO dado que pertence a ele. Irreversivel.
  ///
  /// Postgres: uma transacao unica sobre as 18 tabelas de
  /// `TENANT_OWNED_TABLES`, mais `memberships` e a linha de `tenants`. Ou some
  /// tudo, ou nao some nada, entao uma falha deixa o workspace intacto e o
  /// usuario pode tentar de novo. `users` e global e nunca e tocada: o usuario
  /// pode ser membro de outros workspaces.
  ///
  /// LMDB: o range prefixado do tenant nos 12 sub-dbs de `TENANT_OWNED_DBS`.
  /// `sessions` NAO e prefixado por tenant nesse backend, entao a sessao
  /// sobrevive aqui; ela morre no primeiro request, porque em modo cloud o
  /// `admin_guard` re-resolve a membership a cada requisicao.
  async fn delete_tenant(&self, id: TenantId) -> Result<(), StoreError>;
  ```

- [ ] **Step 1: Escrever os testes que falham**

Em `tests/workspace_it.rs`:

```rust
#[tokio::test]
async fn delete_tenant_removes_every_owned_row_pg() {
    // Cria dois tenants. Semeia, no tenant A e no tenant B, pelo menos:
    // um link, um webhook, um api_token, um domain, um invite, um
    // oidc_config, uma membership.
    // Chama delete_tenant(A).
    // Confirma que TODA linha de A sumiu e que NENHUMA de B sumiu.
    // Confirma que a linha de `users` do dono de A continua existindo.
}

#[tokio::test]
async fn delete_tenant_is_atomic_pg() {
    // Confirma que uma falha no meio nao deixa o tenant meio-apagado.
    // Forma mais simples: apagar um tenant inexistente nao apaga nada de
    // ninguem e nao devolve erro (idempotente), e o estado de B fica intacto.
}

#[tokio::test]
async fn delete_tenant_frees_the_slug_pg() {
    // Cria com slug "acme", apaga, e cria de novo com o mesmo slug: 201.
    // Prova a promessa do spec de que o slug volta a ficar livre.
}
```

O segundo assert de cada teste (o que confirma que o tenant vizinho ficou intacto) é o mais importante do conjunto: apagar demais é a falha perigosa.

- [ ] **Step 2: Rodar, confirmar a falha**

```bash
reset_db; CARGO_BUILD_JOBS=1 cargo test -j1 --test workspace_it 2>&1 | tail -20
```

- [ ] **Step 3: Implementar no Postgres**

Uma transação só. Reaproveite a constante `TENANT_OWNED_TABLES` (`postgres.rs:115`) em vez de repetir a lista, senão uma tabela nova entra no sistema sem entrar no delete:

```rust
async fn delete_tenant(&self, id: TenantId) -> Result<(), StoreError> {
    let mut tx = self.write.begin().await.map_err(StoreError::backend)?;
    // A lista compartilhada com a DDL e o setup de RLS: uma tabela nova
    // passa a ser apagada sem ninguem lembrar de atualizar dois lugares.
    for table in TENANT_OWNED_TABLES {
        sqlx::query(&format!("DELETE FROM {table} WHERE tenant_id = $1"))
            .bind(id.0 as i64)
            .execute(&mut *tx)
            .await
            .map_err(StoreError::backend)?;
    }
    // `memberships` nao esta em TENANT_OWNED_TABLES e precisa ser tratada
    // a parte. `users` e global e nunca e tocada.
    sqlx::query("DELETE FROM memberships WHERE tenant_id = $1")
        .bind(id.0 as i64)
        .execute(&mut *tx)
        .await
        .map_err(StoreError::backend)?;
    sqlx::query("DELETE FROM tenants WHERE id = $1")
        .bind(id.0 as i64)
        .execute(&mut *tx)
        .await
        .map_err(StoreError::backend)?;
    tx.commit().await.map_err(StoreError::backend)?;
    Ok(())
}
```

**Atenção ao RLS:** esta transação não usa `with_write!`, porque precisa apagar de `tenants` e `memberships`, que não são tenant-owned. Confirme que os `DELETE` nas tabelas com `FORCE ROW LEVEL SECURITY` funcionam sem `app.tenant_id` setado; se a policy barrar, use `begin_tenant_tx` para a parte tenant-owned e uma segunda transação para o resto, e **documente por que são duas**. Rode contra a role não-superusuária: com superusuário isso passa e engana.

- [ ] **Step 4: Implementar no LMDB**

Apague o range prefixado do tenant em cada um dos 12 `TENANT_OWNED_DBS`, usando o helper `tprefix` (`lmdb.rs:24-39`) que já monta o prefixo. Uma `write_txn` só.

- [ ] **Step 5: Stubs nos mocks**

`src/domain_router.rs` e `src/webhooks/delivery.rs` têm mocks de `Store`. Siga o que cada um já faz para métodos que não usa (`unimplemented!()` num, `Ok(())` no outro).

- [ ] **Step 6: Gate**

```bash
for t in workspace_it tenant_isolation_it postgres_store_it; do
  reset_db; CARGO_BUILD_JOBS=1 cargo test -j1 --test $t 2>&1 | grep -E "^test result|^error"
done
cargo fmt --check && CARGO_BUILD_JOBS=1 cargo clippy -j1 --all-targets -- -D warnings 2>&1 | tail -2
CARGO_BUILD_JOBS=1 cargo test -j1 --lib 2>&1 | grep -E "^test result"
```

- [ ] **Step 7: Commit**

---

### Task 4: `DELETE /admin/tenants/:id`

**Files:**
- Modify: `src/api/tenants.rs` (handler novo)
- Modify: `src/api/router.rs` (rota, ao lado das duas de workspace na linha 95)
- Test: `tests/workspace_it.rs`

**Interfaces:**
- Consumes: `delete_tenant` (Task 3), `delete_realm` (Task 1), `delete_tenant_data` (Task 2).

- [ ] **Step 1: Escrever os testes que falham**

Um por regra de autorização. Todos em `tests/workspace_it.rs`:

```rust
// 404 em OSS, antes de qualquer checagem de credencial (paridade com
// oss_workspace_endpoints_are_404, que ja existe).
async fn delete_tenant_is_404_in_oss()

// Sem cookie de sessao: 401, nao 404.
async fn delete_tenant_requires_session()

// Membro com papel Member: 403. Com Admin: 403 tambem, porque a regra e
// Owner-only e Admin compartilha escopos com Owner.
async fn delete_tenant_rejects_non_owner()

// Id de um tenant onde o usuario nao e membro: 404, nao 403, para nao
// vazar existencia.
async fn delete_tenant_hides_foreign_tenants()

// Ultimo workspace do usuario: 409, e o tenant continua existindo.
async fn delete_tenant_refuses_the_last_workspace()

// Caminho feliz: Owner com dois workspaces apaga um, recebe 204, o tenant
// sumiu, e /admin/me passa a listar so o outro.
async fn delete_tenant_succeeds_for_the_owner()

// O realm e apagado com o slug certo, e apenas ele.
async fn delete_tenant_deletes_only_its_own_realm()

// Falha no Keycloak nao desfaz a exclusao nem devolve erro: o tenant sumiu,
// a resposta e 204, e sai um warn. Espelha
// create_tenant_survives_ensure_realm_failure, que ja existe.
async fn delete_tenant_survives_delete_realm_failure()
```

- [ ] **Step 2: Rodar, confirmar as falhas**

- [ ] **Step 3: Implementar o handler**

Estrutura, seguindo o que `admin_tenants_create` (`tenants.rs:261-340`) já faz:

1. `if !st.multi_tenant` → `404`.
2. `session_user_id` → `401` se ausente. **Não** aceite token de API: excluir workspace não é operação de automação. Confira como o `admin_guard` distingue sessão de token e siga o mesmo caminho de `admin_tenants_create`, que também exige sessão.
3. `get_membership(user_id, target)` → `None` significa `404` (não vaza existência).
4. `m.role != Role::Owner` → `403`.
5. `list_memberships_for_user(user_id).len() <= 1` → `409`.
6. `store.delete_tenant(target)` → `503` em erro. **Nada foi apagado**, a transação garante.
7. `sink.delete_tenant_data(target.0)` best-effort: erro só loga `warn!`.
8. `keycloak.delete_realm(&slug)` best-effort: erro só loga `warn!` com o slug. **Capture o slug ANTES do passo 6**, senão o tenant já sumiu e você não tem o nome do realm.
9. Invalide o cache do `host_router` para os hosts do tenant, como `admin_tenants_create` faz depois de semear o subdomínio.
10. `204 No Content`.

A ordem 6 → 8 é a decisão do spec e não deve ser invertida: realm apagado com o tenant vivo é um workspace no qual ninguém consegue entrar.

- [ ] **Step 4: Rota**

Em `src/api/router.rs`, junto de `/admin/tenants` (linha 95).

- [ ] **Step 5: Gate e commit**

---

### Task 5: Exclusão no painel

**Files:**
- Modify: `web/src/components/WorkspaceSwitcher.tsx`
- Create: `web/src/components/DeleteWorkspaceDialog.tsx`
- Modify: `web/src/lib/api.ts`, `web/src/lib/queries.ts`
- Modify: `web/src/i18n/en.ts`, `web/src/i18n/pt-BR.ts`
- Test: `web/src/components/DeleteWorkspaceDialog.test.tsx`

- [ ] **Step 1: Escrever os testes que falham**

```tsx
it("mantem o botao de confirmar desabilitado ate o slug bater", ...)
it("chama a API quando o slug digitado confere", ...)
it("nao oferece exclusao para quem nao e owner", ...)
it("mostra a mensagem certa quando a API recusa o ultimo workspace (409)", ...)
```

O primeiro é o que protege o usuário; escreva-o primeiro.

- [ ] **Step 2: Implementar**

Item no menu do `WorkspaceSwitcher`, visível **só quando o papel do workspace atual é `owner`** (o `/admin/me` já devolve `role` em cada membership). O diálogo exige digitar o slug exato; o botão fica desabilitado até bater.

No sucesso, invalida `["me"]` e fecha. O `WorkspaceGate` já sabe levar o usuário ao workspace restante.

Trate `409` com mensagem própria (é o último workspace), `403` com outra, e o resto com a genérica. Siga o padrão de tratamento de erro que `CreateWorkspaceForm.tsx:39-46` já usa.

- [ ] **Step 3: Gate e commit**

```bash
cd web && npm run lint && npm test && npx tsc --noEmit
```

---

### Task 6: Feedback na criação (LUC-139)

**Files:**
- Modify: `web/src/components/CreateWorkspaceForm.tsx`
- Modify: `web/src/i18n/en.ts`, `web/src/i18n/pt-BR.ts`
- Test: `web/src/components/CreateWorkspaceForm.test.tsx`

- [ ] **Step 1: Escrever os testes que falham**

```tsx
it("explica que o login do workspace esta sendo preparado enquanto cria", ...)
it("avisa que esta demorando mais que o normal depois do limite", ...)
```

Use os fake timers do Vitest para o segundo; não faça o teste esperar de verdade.

- [ ] **Step 2: Implementar**

Enquanto `mutation.isPending`, o texto explica que o login do workspace está sendo preparado em outro sistema. Passado o limite (comece com 8 segundos, e deixe o valor numa constante nomeada com comentário dizendo de onde veio), aparece a segunda linha dizendo que está demorando mais que o normal e que recarregar a página é seguro.

**Não fabrique etapas que avançam por timer.** Não existe polling no painel e inventar progresso falso é mentir para o usuário. O texto explica o que está acontecendo; ele não finge saber em que passo está.

- [ ] **Step 3: Gate e commit**

---

### Task 7: Documentação

**Files:**
- Modify: `docs/WORKSPACES.md` e `docs/WORKSPACES.PT_BR.md`, ou o arquivo equivalente que já documenta workspace. **Procure antes de criar**: se não existir, crie os dois com o cabeçalho de troca de idioma que os outros docs usam.

- [ ] **Step 1: Documentar**

Cobrir, nos dois idiomas, em prosa natural de cada um:

- Que a exclusão é **irreversível** e apaga links, cliques, analytics, webhooks, domínios e convites do workspace.
- Que **só o `Owner`** exclui.
- Que **o último workspace não pode ser excluído**.
- Que o **slug volta a ficar disponível** depois da exclusão.
- Que os cliques no **ClickHouse somem de forma eventual**, porque `ALTER TABLE ... DELETE` é mutation assíncrona. Este item não é detalhe: é o que o produto promete sobre dado do cliente.
- Que **o realm do Keycloak é apagado**, e que se essa etapa falhar o workspace já foi excluído e o realm fica órfão, com registro no log.

- [ ] **Step 2: Commit**

---

## Auto-revisão do plano

| Requisito do spec | Task |
|---|---|
| Hard delete de todo dado do tenant | 2, 3 |
| Transação atômica no Postgres | 3 |
| Range do tenant no LMDB | 3 |
| ClickHouse | 2 |
| `memberships` tratada à parte, `users` intocada | 3 |
| Realm apagado depois do commit, best-effort | 4 |
| Só `Owner` | 4 |
| Último workspace bloqueado | 4 |
| Tenant alheio devolve 404, não 403 | 4 |
| 404 em OSS | 4 |
| Confirmação por digitação do slug | 5 |
| Slug volta a ficar livre | 3 (teste), 7 (doc) |
| Feedback na criação sem progresso falso | 6 |
| Docs nos dois idiomas | 7 |

**Riscos conhecidos para quem implementa:**

1. **RLS na Task 3.** A transação apaga de tabelas com `FORCE ROW LEVEL SECURITY` e também de `tenants`/`memberships`, que não têm policy. Se a policy barrar o delete sem `app.tenant_id`, o desenho precisa de duas transações, e isso muda a garantia de atomicidade: documente o que ficou. **Teste contra a role não-superusuária**, porque com superusuário RLS não se aplica e o bug fica invisível.
2. **Capturar o slug antes de apagar** (Task 4, passo 8). Depois do delete o tenant sumiu e não há de onde ler o nome do realm.
3. **O assert que mais importa em toda task de exclusão é o do vizinho intacto.** Apagar demais é o modo de falha perigoso, e um teste que só confirma que o alvo sumiu passaria com um `DELETE` sem `WHERE`.
