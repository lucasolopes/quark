# Tijolo 10 — Busca server-side (Postgres) com paginação — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expor busca server-side paginada nos links do quark via `GET /admin/links?q=<termo>`, com o Postgres fazendo `ILIKE` em url+alias e o LMDB retornando "não suportado" (→ 501 → o painel cai pro filtro client-side atual).

**Architecture:** Novo método `Store::search_links` no trait (Postgres implementa keyset+ILIKE; LMDB retorna `StoreError::Unsupported`). O handler `admin_links_list` passa a decidir entre `list_links` (sem `q`) e `search_links` (com `q`), mapeando `Unsupported`→501. No frontend, `useLinks(q)` chaveia a query por `q`, faz debounce ~300ms e, ao receber 501, cai pro filtro client-side sobre a lista base já carregada.

**Tech Stack:** Rust (axum, sqlx, heed, async-trait, serial_test); React 19 + Vite + TanStack Query + Vitest.

## Global Constraints

- Busca server-side é **Postgres-only**; LMDB retorna `StoreError::Unsupported` (→ endpoint 501 → front client-side). Não implementar busca no LMDB.
- Postgres casa `url` **e** `alias` (LEFT JOIN), **case-insensitive** (`ILIKE`), keyset por `id` (`id > after`).
- Curingas `%`, `_`, `\` do termo são **escapados** (busca literal, não wildcard SQL).
- `q` **ausente ou vazio (após trim)** = listagem atual, inalterada. Redirect/leitura intocados.
- Testes de Postgres são **gated** por `QUARK_TEST_DATABASE_URL` (pulam quando ausente).
- Resposta do endpoint é a mesma de hoje: `{ "links": [...], "next_after": <id|null> }`, `next_after` só em página cheia.
- Tasks de UI seguem a skill **frontend-design** e as heurísticas de Nielsen.

---

### Task 1: `Store::search_links` — trait + Postgres + LMDB

**Files:**
- Modify: `src/store/mod.rs` (variante `StoreError::Unsupported` + braço no `Display` + assinatura no trait)
- Modify: `src/store/postgres.rs` (impl `search_links` + helper `like_escape`)
- Modify: `src/store/lmdb.rs` (impl `search_links` → `Err(Unsupported)`)
- Test: `src/store/lmdb.rs` (teste unit no módulo `#[cfg(test)]` já existente) e `tests/search_it.rs` (novo, gated Postgres)

**Interfaces:**
- Produces:
  - `StoreError::Unsupported` (variante nova).
  - `async fn search_links(&self, q: &str, after: Option<u64>, limit: usize) -> Result<Vec<(u64, Record)>, StoreError>` no trait `Store` (implementado por LMDB e Postgres).
  - `fn like_escape(q: &str) -> String` (livre, em `postgres.rs`).
- Consumes: `Record { url, expiry, created }`, `StoreError::backend`, padrão de `list_links` (keyset `id > $1`).

- [ ] **Step 1: Adicionar a variante `StoreError::Unsupported` + Display**

Em `src/store/mod.rs`, no enum (linhas 17-22) e no `Display` (23-31):

```rust
pub enum StoreError {
    Db(heed::Error),
    Serde(serde_json::Error),
    Backend(String),
    IdSpaceExhausted,
    /// Operação não suportada por este backend (ex.: busca server-side no LMDB).
    Unsupported,
}
impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Db(e) => write!(f, "db: {e}"),
            StoreError::Serde(e) => write!(f, "serde: {e}"),
            StoreError::Backend(s) => write!(f, "backend: {s}"),
            StoreError::IdSpaceExhausted => write!(f, "espaço de id esgotado"),
            StoreError::Unsupported => write!(f, "operação não suportada por este backend"),
        }
    }
}
```

- [ ] **Step 2: Declarar `search_links` no trait**

Em `src/store/mod.rs`, logo após `list_links` (linha 71-75) e antes de `list_aliases`:

```rust
    /// Busca server-side paginada (keyset por id). Casa `url`/`alias`,
    /// case-insensitive, termo literal. Backends sem busca retornam
    /// `Err(StoreError::Unsupported)`.
    async fn search_links(
        &self,
        q: &str,
        after: Option<u64>,
        limit: usize,
    ) -> Result<Vec<(u64, Record)>, StoreError>;
```

- [ ] **Step 3: LMDB retorna `Unsupported`**

Em `src/store/lmdb.rs`, dentro do `impl Store for LmdbStore`, junto dos outros métodos de listagem:

```rust
    async fn search_links(
        &self,
        _q: &str,
        _after: Option<u64>,
        _limit: usize,
    ) -> Result<Vec<(u64, Record)>, StoreError> {
        // LMDB não tem índice de texto; busca server-side é recurso do Postgres.
        Err(StoreError::Unsupported)
    }
```

- [ ] **Step 4: Escrever o teste unit do LMDB (falhando)**

No módulo de testes de `src/store/lmdb.rs` (o `#[cfg(test)] mod tests` já existente), adicione:

```rust
    #[tokio::test]
    async fn search_links_is_unsupported_on_lmdb() {
        let dir = tempfile::tempdir().unwrap();
        let store = LmdbStore::open(dir.path(), 0).unwrap();
        let r = store.search_links("qualquer", None, 10).await;
        assert!(matches!(r, Err(StoreError::Unsupported)));
    }
```

Nota: use a MESMA forma de abrir o store dos outros testes do arquivo (confira a assinatura de `LmdbStore::open` no topo do `mod tests` e replique — não invente parâmetros).

- [ ] **Step 5: Rodar o teste unit — deve FALHAR (compilação: método/variante ainda não existem se algum step foi pulado)**

Run: `cargo test --lib store::lmdb::tests::search_links_is_unsupported_on_lmdb`
Expected: compila e PASSA depois dos steps 1-3 (o método já retorna `Unsupported`). Se algum step foi pulado, falha de compilação nomeando `search_links`/`Unsupported`.

- [ ] **Step 6: Implementar `like_escape` + `search_links` no Postgres**

Em `src/store/postgres.rs`, adicione o helper livre (fora do `impl`, topo do arquivo, após os `use`):

```rust
/// Escapa os curingas do `LIKE`/`ILIKE` (escape char padrão = `\`) para que o
/// termo do usuário seja tratado literalmente. Ordem importa: escapa a `\` antes.
fn like_escape(q: &str) -> String {
    q.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}
```

E o método dentro do `impl Store for PostgresStore` (logo após `list_links`):

```rust
    async fn search_links(
        &self,
        q: &str,
        after: Option<u64>,
        limit: usize,
    ) -> Result<Vec<(u64, Record)>, StoreError> {
        let pattern = format!("%{}%", like_escape(q));
        let rows = sqlx::query(
            "SELECT DISTINCT l.id, l.url, l.expiry, l.created \
             FROM links l LEFT JOIN aliases a ON a.id = l.id \
             WHERE ($1::bigint IS NULL OR l.id > $1) \
               AND (l.url ILIKE $2 OR a.alias ILIKE $2) \
             ORDER BY l.id LIMIT $3",
        )
        .bind(after.map(|a| a as i64))
        .bind(&pattern)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(StoreError::backend)?;
        let mut out = Vec::new();
        for r in rows {
            let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
            let url: String = r.try_get("url").map_err(StoreError::backend)?;
            let expiry: Option<i64> = r.try_get("expiry").map_err(StoreError::backend)?;
            let created: i64 = r.try_get("created").map_err(StoreError::backend)?;
            out.push((
                id as u64,
                Record {
                    url,
                    expiry: expiry.map(|v| v as u64),
                    created: created as u64,
                },
            ));
        }
        Ok(out)
    }
```

- [ ] **Step 7: Escrever os testes gated do Postgres (falhando)**

Crie `tests/search_it.rs`. Use o MESMO padrão de gate dos outros testes de integração Postgres do repo (confira `tests/` — provavelmente lê `QUARK_TEST_DATABASE_URL` e faz `return` cedo quando ausente, ou usa um helper comum). Replique esse helper de setup (conexão + `PostgresStore::connect`/migração) idêntico ao arquivo de integração Postgres existente. Testes:

```rust
// Semeia 3 links: github.com/rust, example.com, rust-lang.org com alias "rust".
// Busca "rust" casa por url (rust-lang.org) E por alias (o link com alias "rust").
#[tokio::test]
async fn search_matches_url_and_alias() {
    let Some(store) = pg_store().await else { return }; // gate: pula sem DATABASE_URL
    // put_alias_and_link / put_link conforme o helper — ver como os outros ITs semeiam
    let ids = seed_links(&store, &[
        ("https://github.com/rust-lang", None),
        ("https://example.com", None),
        ("https://docs.rs", Some("rust")), // alias "rust", url NÃO contém rust
    ]).await;
    let hits = store.search_links("rust", None, 50).await.unwrap();
    let urls: Vec<&str> = hits.iter().map(|(_, r)| r.url.as_str()).collect();
    assert!(urls.iter().any(|u| u.contains("github.com/rust-lang")), "casa url");
    assert!(hits.iter().any(|(id, _)| *id == ids[2]), "casa alias 'rust'");
    assert!(!urls.iter().any(|u| *u == "https://example.com"), "não casa example");
}

// Curinga literal: "%" no termo NÃO vira wildcard SQL.
#[tokio::test]
async fn search_escapes_wildcards() {
    let Some(store) = pg_store().await else { return };
    seed_links(&store, &[
        ("https://ex.com/50%off", None),
        ("https://ex.com/504", None),
    ]).await;
    let hits = store.search_links("50%", None, 50).await.unwrap();
    let urls: Vec<&str> = hits.iter().map(|(_, r)| r.url.as_str()).collect();
    assert!(urls.iter().any(|u| u.contains("50%off")), "casa o literal 50%off");
    assert!(!urls.iter().any(|u| u.contains("504")), "NÃO casa 504 (% não é wildcard)");
}

// Keyset: after corta os ids <= after.
#[tokio::test]
async fn search_keyset_pagination() {
    let Some(store) = pg_store().await else { return };
    let ids = seed_links(&store, &[
        ("https://ex.com/alfa", None),
        ("https://ex.com/alfa2", None),
        ("https://ex.com/alfa3", None),
    ]).await;
    let page1 = store.search_links("alfa", None, 2).await.unwrap();
    assert_eq!(page1.len(), 2);
    let after = page1.last().unwrap().0;
    let page2 = store.search_links("alfa", Some(after), 2).await.unwrap();
    assert!(page2.iter().all(|(id, _)| *id > after), "página 2 só tem id > after");
    assert!(page2.iter().any(|(id, _)| *id == ids[2]));
}
```

Nota importante: os nomes `pg_store()` / `seed_links()` acima são placeholders do **helper que já existe** no IT de Postgres do repo. Antes de escrever, ABRA o arquivo de integração Postgres existente (ex.: `tests/*postgres*` ou o que semeia links num pool real) e reuse os helpers reais dele; se não houver helper reutilizável, escreva o setup inline idêntico ao daquele arquivo (mesma env var, mesma criação de schema). Não crie um mecanismo de gate novo.

- [ ] **Step 8: Rodar os testes gated — verificar que passam com Postgres no ar (e pulam sem ele)**

Run (sem DB, devem "passar" pulando): `cargo test --test search_it`
Run (com DB): `QUARK_TEST_DATABASE_URL=postgres://quark:quark@127.0.0.1:5433/quark cargo test --test search_it`
Expected: com DB, 3 testes PASSAM; sem DB, retornam cedo (verde). Use a porta/credencial reais do compose de dev (confira `docker-compose.yml`).

- [ ] **Step 9: Rodar a suíte de lib + fmt/clippy**

Run: `cargo test --lib && cargo fmt --check && cargo clippy --all-targets -- -D warnings`
Expected: verde. (Se clippy reclamar de `search_links` não usado no LMDB por `_q` etc., já está prefixado com `_`.)

- [ ] **Step 10: Commit**

```bash
git add src/store/mod.rs src/store/postgres.rs src/store/lmdb.rs tests/search_it.rs
git commit -m "feat(store): search_links — Postgres ILIKE(url+alias) keyset + escape de curinga; LMDB→Unsupported"
```

---

### Task 2: Endpoint `GET /admin/links?q=` — roteia search vs list, 501 em Unsupported

**Files:**
- Modify: `src/api.rs` (`struct ListParams` + `admin_links_list`)
- Test: `tests/api_it.rs` (novo caso: LMDB + `?q=` → 501)

**Interfaces:**
- Consumes: `Store::search_links` (Task 1), `StoreError::Unsupported` (Task 1), `admin_guard`, `codec::to_base62`, `permute::encode`, `LinkRow`.
- Produces: comportamento HTTP de `GET /admin/links?q=&after=&limit=`.

- [ ] **Step 1: Escrever o teste de API (falhando) — LMDB + q → 501**

Em `tests/api_it.rs`, no padrão dos testes que montam o app com `QUARK_ADMIN_TOKEN` (reuse o helper existente, ex.: `app_with_admin()` / a forma como os testes de `/admin/links` montam o router e mandam o header `x-admin-token`):

```rust
#[tokio::test]
async fn admin_links_search_on_lmdb_returns_501() {
    let (app, token) = app_with_admin().await; // reuse o helper real do arquivo
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/admin/links?q=abc")
                .header("x-admin-token", &token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_IMPLEMENTED); // 501
}
```

Nota: os nomes `app_with_admin()`/imports (`Request`, `Body`, `ServiceExt::oneshot`, `StatusCode`) devem espelhar EXATAMENTE os já usados no `tests/api_it.rs` — copie o preâmbulo de outro teste de `/admin/links` do mesmo arquivo. O app de teste usa LMDB (store default dos ITs), logo `search_links`→`Unsupported`→501.

- [ ] **Step 2: Rodar — deve FALHAR (hoje `?q=` é ignorado, cai no list e retorna 200)**

Run: `cargo test --test api_it admin_links_search_on_lmdb_returns_501`
Expected: FAIL — recebido `200 OK`, esperado `501`.

- [ ] **Step 3: Adicionar `q` ao `ListParams`**

Em `src/api.rs` (linhas 399-402):

```rust
struct ListParams {
    after: Option<u64>,
    limit: Option<usize>,
    q: Option<String>,
}
```

- [ ] **Step 4: Rotear search vs list no `admin_links_list`**

Em `src/api.rs`, substitua APENAS o bloco que obtém `links` (linhas 423-427) por:

```rust
    let limit = p.limit.unwrap_or(50).clamp(1, 500);
    let q = p.q.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let links = match q {
        Some(term) => match st.store.search_links(term, p.after, limit).await {
            Ok(l) => l,
            // Backend sem busca server-side (LMDB): sinaliza ao painel cair
            // pro filtro client-side.
            Err(StoreError::Unsupported) => {
                return StatusCode::NOT_IMPLEMENTED.into_response()
            }
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        },
        None => match st.store.list_links(p.after, limit).await {
            Ok(l) => l,
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        },
    };
```

O resto do handler (alias_map, next_after, rows, Json) fica **idêntico** — a enriquecimento de alias e o `next_after` só em página cheia se aplicam aos dois caminhos. Garanta que `StoreError` está no escopo (`use crate::store::StoreError;` no topo de `api.rs`; se já houver `use crate::store::...`, adicione `StoreError` à lista).

- [ ] **Step 5: Rodar o teste de API — deve PASSAR**

Run: `cargo test --test api_it admin_links_search_on_lmdb_returns_501`
Expected: PASS (501).

- [ ] **Step 6: Rodar a suíte de API inteira + clippy (garantir que list sem q não regrediu)**

Run: `cargo test --test api_it && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: verde (todos os testes de `/admin/links` sem `q` continuam passando).

- [ ] **Step 7: Commit**

```bash
git add src/api.rs tests/api_it.rs
git commit -m "feat(api): GET /admin/links?q= roteia p/ search_links; Unsupported→501; sem q inalterado"
```

---

### Task 3: Frontend — busca server-side debounced com fallback client-side em 501

**Files:**
- Modify: `web/src/lib/api.ts` (`listLinks` aceita `q`)
- Modify: `web/src/lib/queries.ts` (`useLinks(q?)` chaveada por `q`)
- Modify: `web/src/routes/Links.tsx` (input debounced → `q`; modo servidor/cliente; estado vazio)
- Create/confirm: `web/src/hooks/useDebounce.ts` (se não existir)
- Test: `web/src/lib/queries.test.tsx` (ou o arquivo de teste de queries existente) + teste de fallback

**Interfaces:**
- Consumes: endpoint `GET /admin/links?q=&after=&limit=` (Task 2), `ApiError { status }` (já existe em `api.ts`, com `status: number`).
- Produces: `useLinks(q?: string)` (server-side quando `q` e backend suporta; client-side quando 501).

Use a skill **frontend-design** para esta task (UI/UX Nielsen). Diretrizes do design (spec §3):
- Manter a **lista base** (`useLinks()` sem `q`) sempre carregada e cacheada — é a fonte do fallback client-side.
- Ao digitar, um estado `q` é atualizado; um `dq` = debounce ~300ms de `q` dispara a busca.
- Enquanto o backend suportar busca (não recebeu 501), com `dq` não-vazio usa-se a **busca server-side** (`useLinks(dq)`, paginada, "carregar mais").
- Ao receber **ApiError status 501** na busca, marca-se o modo como client-side pelo resto da sessão e passa-se a **filtrar a lista base** por `dq` (comportamento atual do painel).
- Estado vazio de busca: `nenhum link encontrado para "<dq>"`.
- Nada de rodar duas buscas server-side simultâneas; a lista base é a única query extra (barata, keyset).

- [ ] **Step 1: `listLinks` aceita `q` (implementação)**

Em `web/src/lib/api.ts`, no client tipado, estenda os params de `listLinks` para incluir `q?: string` e inclua no query string quando não-vazio. Padrão (ajuste aos nomes reais do arquivo):

```ts
export async function listLinks(
  params: { after?: number; limit?: number; q?: string } = {},
): Promise<LinksPage> {
  const sp = new URLSearchParams();
  if (params.after != null) sp.set("after", String(params.after));
  if (params.limit != null) sp.set("limit", String(params.limit));
  if (params.q && params.q.trim() !== "") sp.set("q", params.q.trim());
  const qs = sp.toString();
  return request<LinksPage>(`/admin/links${qs ? `?${qs}` : ""}`);
}
```

Reuse o helper `request`/`fetch` já existente (que lança `ApiError` com `.status`); NÃO reimplemente o fetch. Confira o nome real da função e do tipo de página (`LinksPage`) no arquivo e mantenha-os.

- [ ] **Step 2: Escrever o teste (falhando) — busca chama a API com `q`**

No arquivo de teste de `api.ts`/`queries` (Vitest; siga o padrão existente com `vi.fn()`/`msw`/mock de `fetch` já usado no projeto):

```ts
it("listLinks inclui q no querystring", async () => {
  const spy = mockFetchOnce({ links: [], next_after: null }); // helper real do projeto
  await listLinks({ q: "git", limit: 50 });
  const url = spy.mock.calls[0][0] as string;
  expect(url).toContain("q=git");
});
```

Use o MESMO mecanismo de mock de fetch dos testes existentes (confira o `*.test.ts(x)` de api/queries e replique `mockFetchOnce`/setup — não introduza msw se o projeto usa mock manual, nem vice-versa).

- [ ] **Step 3: Rodar — deve FALHAR**

Run: `cd web && npx vitest run src/lib` (ou o caminho do teste)
Expected: FAIL (o querystring ainda não inclui `q` se o Step 1 foi pulado; após Step 1, este teste passa — ordene: escreva teste, veja passar por Step 1 já feito, então é um teste de regressão. Se preferir TDD estrito, faça Step 2 antes do Step 1 e veja falhar).

- [ ] **Step 4: `useDebounce` (confirmar/criar)**

Se `web/src/hooks/useDebounce.ts` não existir, crie:

```ts
import { useEffect, useState } from "react";

export function useDebounce<T>(value: T, delayMs = 300): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setDebounced(value), delayMs);
    return () => clearTimeout(t);
  }, [value, delayMs]);
  return debounced;
}
```

- [ ] **Step 5: `useLinks(q?)` chaveada por `q` (implementação)**

Em `web/src/lib/queries.ts`, torne `useLinks` parametrizável por `q`, mantendo `useInfiniteQuery` (keyset). O `queryKey` inclui `q`; a `queryFn` passa `q` a `listLinks`; `getNextPageParam` continua retornando `undefined` quando `lastPage.links.length < limit`. Padrão:

```ts
export function useLinks(q?: string) {
  const term = q?.trim() ?? "";
  return useInfiniteQuery({
    queryKey: ["links", term],
    queryFn: ({ pageParam }) =>
      listLinks({ after: pageParam as number | undefined, limit: PAGE_SIZE, q: term || undefined }),
    initialPageParam: undefined as number | undefined,
    getNextPageParam: (lastPage) =>
      lastPage.links.length < PAGE_SIZE ? undefined : lastPage.next_after ?? undefined,
    retry: (count, err) => !(err instanceof ApiError && err.status === 501) && count < 3,
  });
}
```

`retry` NÃO deve reintentar em 501 (é resposta definitiva do backend). Importe `ApiError` de `@/lib/api`. Mantenha `PAGE_SIZE` igual ao já usado (não invente um novo).

- [ ] **Step 6: `Links.tsx` — input debounced, modo servidor/cliente, estado vazio (frontend-design)**

Em `web/src/routes/Links.tsx`:
- `const base = useLinks();` (lista base, sempre — fonte do fallback).
- `const [q, setQ] = useState(""); const dq = useDebounce(q, 300);`
- `const [clientMode, setClientMode] = useState(false);`
- `const search = useLinks(clientMode ? undefined : (dq || undefined));` com a query de busca **desabilitada** quando `dq === "" || clientMode` (use `enabled` do TanStack — abra a assinatura atual de `useLinks` e adicione um param de opções OU controle via `enabled` dentro do hook; a forma mais limpa é o hook aceitar `{ enabled }`).
- `useEffect`: se `search.error instanceof ApiError && search.error.status === 501` → `setClientMode(true)`.
- Linhas exibidas:
  - `dq === ""` → páginas de `base` achatadas (tabela normal, "carregar mais" de `base`).
  - `clientMode` → páginas de `base` achatadas **filtradas** por `dq` (url/alias/code, case-insensitive) — o filtro client-side que o painel já tem hoje.
  - senão → páginas de `search` achatadas ("carregar mais" de `search`).
- Estado vazio quando a lista exibida está vazia e `dq !== ""`: texto `nenhum link encontrado para "{dq}"`.
- O input mantém `role="searchbox"`/`aria-label` já existente; mostrar um spinner sutil enquanto `search.isFetching` (feedback — Nielsen "visibilidade do status").

Mantenha o visual/acessibilidade no padrão shadcn/Tailwind já usado no arquivo. NÃO reescreva a tabela; só troque a origem das linhas e adicione o estado vazio + spinner.

- [ ] **Step 7: Escrever o teste de fallback (falhando) — 501 → filtro client-side**

Em `web/src/routes/Links.test.tsx` (ou onde os componentes são testados; use `@testing-library/react` + `QueryClientProvider` no padrão do projeto):

```tsx
it("cai pro filtro client-side quando a busca retorna 501", async () => {
  // base list: 2 links; a chamada com ?q= responde 501
  mockFetchSequence([
    { match: (u) => !u.includes("q="), json: { links: [
      { id: 1, code: "aaa", url: "https://github.com/x", created: 0 },
      { id: 2, code: "bbb", url: "https://example.com", created: 0 },
    ], next_after: null } },
    { match: (u) => u.includes("q="), status: 501, json: {} },
  ]);
  render(<Links />, { wrapper: withProviders });
  const box = await screen.findByRole("searchbox");
  fireEvent.change(box, { target: { value: "github" } });
  // após debounce + 501, filtra a base client-side: só o github aparece
  expect(await screen.findByText(/github\.com\/x/)).toBeInTheDocument();
  expect(screen.queryByText(/example\.com/)).not.toBeInTheDocument();
});
```

`mockFetchSequence`/`withProviders` são placeholders — use os helpers reais de teste do projeto (abra um `*.test.tsx` existente de rota e replique o setup de providers/mock; se o projeto usa fake timers pro debounce, avance os timers com `vi.advanceTimersByTime(300)` em vez de esperar tempo real).

- [ ] **Step 8: Rodar os testes de frontend**

Run: `cd web && npx vitest run`
Expected: PASS (busca inclui `q`; fallback 501 filtra client-side).

- [ ] **Step 9: typecheck + lint + build**

Run: `cd web && npm run typecheck && npm run lint && npm run build`
Expected: verde. (Atenção ao `erasableSyntaxOnly` do TS6 — sem parameter properties; e ao oxlint.)

- [ ] **Step 10: Commit**

```bash
git add web/src/lib/api.ts web/src/lib/queries.ts web/src/routes/Links.tsx web/src/hooks/useDebounce.ts web/src/routes/Links.test.tsx web/src/lib/queries.test.tsx
git commit -m "feat(web): busca server-side debounced com fallback client-side em 501"
```

---

## Self-Review

**1. Spec coverage:**
- Spec §1 (`Store::search_links` Postgres ILIKE url+alias keyset + escape; LMDB Unsupported; variante `Unsupported`) → Task 1. ✅
- Spec §2 (endpoint `?q=`: q vazio→list; q→search; Unsupported→501; 503 outros; resposta+next_after) → Task 2. ✅
- Spec §3 (frontend `useLinks(q)` debounce 300ms server-side; fallback client-side em 501; estado vazio) → Task 3. ✅
- Critérios 1-3 (Postgres filtra, keyset, escape) → Task 1 Steps 7-8. Critério 2 (LMDB→501) → Task 2 Step 1. Critério 4 (front) → Task 3 Steps 7-8. Critério 5 (tudo verde) → Steps de fmt/clippy/typecheck/lint/build. ✅

**2. Placeholder scan:** os nomes marcados como "placeholder" (helpers de teste `pg_store`/`seed_links`/`app_with_admin`/`mockFetchOnce`/`withProviders`) são referências explícitas a helpers **já existentes no repo** que o implementador deve localizar e reusar — cada um vem com instrução de onde encontrar. Não são TODOs de lógica de produção; o código de produção está completo.

**3. Type consistency:** `search_links(&self, q: &str, after: Option<u64>, limit: usize) -> Result<Vec<(u64, Record)>, StoreError>` idêntico em trait/Postgres/LMDB e no consumo do handler. `StoreError::Unsupported` usado em Task 1 (def) e Task 2 (match). `ApiError.status` (number) usado em `retry` e no effect de fallback. `useLinks(q?)` definido na Task 3 Step 5 e consumido no Step 6. ✅
