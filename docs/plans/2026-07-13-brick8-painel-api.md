# Tijolo 8 — API do painel (plano de implementação)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Dar ao quark a API que um painel web externo (SPA) consome — listar/editar/apagar links — sob `/admin/*`, sem tocar no redirect.

**Architecture:** Novos métodos no `trait Store` (list/delete, LMDB+Postgres); novos handlers `/admin/links` (GET/DELETE/PATCH) reusando o `admin_guard` e o `QUARK_ADMIN_TOKEN` existentes; um `Cache::invalidate(id)` novo (L1+L2) para PATCH/DELETE refletirem no redirect; CORS opt-in via `QUARK_CORS_ORIGINS`; e um `docker-compose.yml` para dev/self-host.

**Tech Stack:** Rust 2021, axum, tower-http (CorsLayer — dependência NOVA), heed (LMDB range/iter), sqlx (Postgres), moka, redis, serde_json.

## Global Constraints

- Nada novo no caminho de redirect/leitura — só `/admin/*` e o CORS em `POST /`.
- Auth reusa `QUARK_ADMIN_TOKEN` + `admin_guard` (`x-admin-token`; sem token → 404; errado → 401). Sem sessão/cookie.
- Paginação **keyset por id** (não offset); `limit` default 50, teto 500.
- `code` sempre recomputado (`to_base62(encode(id, key))`); nunca armazenado.
- `PATCH`/`DELETE` invalidam o cache (L1 + L2) do id afetado.
- **Sem contagem de cliques** na lista (evita N queries no ClickHouse); alias enriquecido com **um** `list_aliases` por página.
- Métodos novos de `Store` implementados em **LMDB e Postgres**; testes de Postgres **gated** por `QUARK_TEST_DATABASE_URL` (sem a env, pulam; sempre compilam).
- `fmt`/`clippy -D warnings` limpos.

---

## File Structure

- `src/store/mod.rs` — 4 métodos novos no `trait Store`.
- `src/store/lmdb.rs` — impl (range keyset, iter aliases, delete) + testes in-module.
- `src/store/postgres.rs` — impl.
- `src/cache/mod.rs` — `CacheTier::invalidate` no trait; `Cache::invalidate(id)`; fakes de teste atualizados.
- `src/cache/valkey.rs` — `ValkeyTier::invalidate` (DEL).
- `src/api.rs` — handlers `admin_links_list` / `admin_link_delete` / `admin_link_patch`; `ListParams`; rotas; `router_with_cors` + `parse_cors_origins`.
- `Cargo.toml` — dependência `tower-http` (feature `cors`).
- `docker-compose.yml` (novo) — quark + postgres + valkey + clickhouse.
- `README.md` — seção do docker-compose + endpoints/env novos.
- `tests/api_it.rs` — testes de integração dos endpoints.

---

## Task 1: `Store` — list_links / list_aliases / delete_link / delete_alias

**Files:**
- Modify: `src/store/mod.rs`, `src/store/lmdb.rs`, `src/store/postgres.rs`
- Test: in-module em `src/store/lmdb.rs`

**Interfaces:**
- Produces no `trait Store`:
  - `async fn list_links(&self, after: Option<u64>, limit: usize) -> Result<Vec<(u64, Record)>, StoreError>` — keyset: retorna links com id > `after` (ou do início se `None`), ordenados por id, no máximo `limit`.
  - `async fn list_aliases(&self) -> Result<Vec<(String, u64)>, StoreError>` — todos os pares alias→id.
  - `async fn delete_link(&self, id: u64) -> Result<(), StoreError>` — idempotente.
  - `async fn delete_alias(&self, alias: &str) -> Result<(), StoreError>` — idempotente.

- [ ] **Step 1: Declarar no trait**

Em `src/store/mod.rs`, dentro de `pub trait Store`, após `list_blocked_domains`:

```rust
    async fn list_links(&self, after: Option<u64>, limit: usize) -> Result<Vec<(u64, Record)>, StoreError>;
    async fn list_aliases(&self) -> Result<Vec<(String, u64)>, StoreError>;
    async fn delete_link(&self, id: u64) -> Result<(), StoreError>;
    async fn delete_alias(&self, alias: &str) -> Result<(), StoreError>;
```

- [ ] **Step 2: Teste que falha (LMDB)**

No `#[cfg(test)] mod tests` de `src/store/lmdb.rs`, adicionar:

```rust
    #[tokio::test]
    async fn list_delete_links_e_aliases() {
        let dir = tempfile::tempdir().unwrap();
        let s = LmdbStore::open_with_node_id(dir.path(), None).unwrap();
        let rec = |u: &str| Record { url: u.into(), expiry: None, created: 0 };
        for id in 1..=5u64 {
            s.put_link(id, &rec(&format!("https://e{id}.com"))).await.unwrap();
        }
        s.put_alias_and_link("promo", 10, &rec("https://promo.com")).await.unwrap();

        // keyset: página 1 (após None, limit 3) => ids 1,2,3
        let p1 = s.list_links(None, 3).await.unwrap();
        assert_eq!(p1.iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![1, 2, 3]);
        // página 2 (após 3, limit 3) => ids 4,5,10
        let p2 = s.list_links(Some(3), 3).await.unwrap();
        assert_eq!(p2.iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![4, 5, 10]);

        // aliases
        let al = s.list_aliases().await.unwrap();
        assert_eq!(al, vec![("promo".to_string(), 10u64)]);

        // delete
        s.delete_link(2).await.unwrap();
        assert!(s.get_link(2).await.unwrap().is_none());
        s.delete_alias("promo").await.unwrap();
        assert_eq!(s.get_alias("promo").await.unwrap(), None);
    }
```

- [ ] **Step 3: Rodar e ver falhar**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --lib store::lmdb::tests::list_delete`
Expected: FAIL na compilação (métodos não existem).

- [ ] **Step 4: Implementar no LMDB**

Em `src/store/lmdb.rs`, adicionar `use std::ops::Bound;` no topo (junto aos outros `use`). No `impl Store for LmdbStore`, adicionar:

```rust
    async fn list_links(&self, after: Option<u64>, limit: usize) -> Result<Vec<(u64, Record)>, StoreError> {
        let rtxn = self.env.read_txn()?;
        let start = match after {
            Some(a) => Bound::Excluded(a),
            None => Bound::Unbounded,
        };
        let range = (start, Bound::Unbounded);
        let mut out = Vec::new();
        for item in self.links.range(&rtxn, &range)?.take(limit) {
            let (id, bytes) = item?;
            out.push((id, serde_json::from_slice(bytes)?));
        }
        Ok(out)
    }

    async fn list_aliases(&self) -> Result<Vec<(String, u64)>, StoreError> {
        let rtxn = self.env.read_txn()?;
        let mut out = Vec::new();
        for item in self.aliases.iter(&rtxn)? {
            let (alias, id) = item?;
            out.push((alias.to_string(), id));
        }
        Ok(out)
    }

    async fn delete_link(&self, id: u64) -> Result<(), StoreError> {
        let mut wtxn = self.env.write_txn()?;
        self.links.delete(&mut wtxn, &id)?;
        wtxn.commit()?;
        Ok(())
    }

    async fn delete_alias(&self, alias: &str) -> Result<(), StoreError> {
        let mut wtxn = self.env.write_txn()?;
        self.aliases.delete(&mut wtxn, alias)?;
        wtxn.commit()?;
        Ok(())
    }
```

- [ ] **Step 5: Rodar e ver passar (LMDB)**

Run: `cargo test --lib store::lmdb::tests::list_delete`
Expected: PASS.

- [ ] **Step 6: Implementar no Postgres**

Em `src/store/postgres.rs`, no `impl Store for PostgresStore`, adicionar:

```rust
    async fn list_links(&self, after: Option<u64>, limit: usize) -> Result<Vec<(u64, Record)>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, url, expiry, created FROM links \
             WHERE ($1::bigint IS NULL OR id > $1) ORDER BY id LIMIT $2",
        )
        .bind(after.map(|a| a as i64))
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
                Record { url, expiry: expiry.map(|v| v as u64), created: created as u64 },
            ));
        }
        Ok(out)
    }

    async fn list_aliases(&self) -> Result<Vec<(String, u64)>, StoreError> {
        let rows = sqlx::query("SELECT alias, id FROM aliases")
            .fetch_all(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        let mut out = Vec::new();
        for r in rows {
            let alias: String = r.try_get("alias").map_err(StoreError::backend)?;
            let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
            out.push((alias, id as u64));
        }
        Ok(out)
    }

    async fn delete_link(&self, id: u64) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM links WHERE id = $1")
            .bind(id as i64)
            .execute(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        Ok(())
    }

    async fn delete_alias(&self, alias: &str) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM aliases WHERE alias = $1")
            .bind(alias)
            .execute(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        Ok(())
    }
```

- [ ] **Step 7: Suíte de lib + fmt/clippy + commit**

Run: `cargo test --lib && cargo fmt && cargo clippy --all-targets -- -D warnings`
Expected: tudo verde/limpo.

```bash
git add src/store/
git commit -m "feat(store): list_links (keyset) / list_aliases / delete_link / delete_alias (LMDB + Postgres)"
```

---

## Task 2: Invalidação de cache (L1 + L2)

**Files:**
- Modify: `src/cache/mod.rs` (trait `CacheTier`, `Cache::invalidate`, fakes de teste)
- Modify: `src/cache/valkey.rs` (`ValkeyTier::invalidate`)

**Interfaces:**
- Produces:
  - No trait `CacheTier`: `async fn invalidate(&self, id: u64) -> Result<(), TierError>;`
  - `Cache::invalidate(&self, id: u64)` — remove do L1 (moka) e, best-effort sob breaker/timeout, do L2.

- [ ] **Step 1: Teste que falha**

No `#[cfg(test)] mod tests` de `src/cache/mod.rs`, adicionar:

```rust
    #[tokio::test]
    async fn invalidate_remove_do_l1() {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn Store> = Arc::new(LmdbStore::open(dir.path()).unwrap());
        store.put_link(1, &rec("u1")).await.unwrap();
        let c = Cache::new(store.clone(), 1000);
        assert_eq!(c.get(1).await.unwrap().unwrap().url, "u1"); // popula L1
        // apaga no store e invalida o cache: próximo get NÃO pode servir do L1
        store.delete_link(1).await.unwrap();
        c.invalidate(1).await;
        assert!(c.get(1).await.unwrap().is_none());
    }
```

- [ ] **Step 2: Rodar e ver falhar**

Run: `cargo test --lib cache::tests::invalidate`
Expected: FAIL — `invalidate` não existe (e o trait não tem o método).

- [ ] **Step 3: Adicionar `invalidate` ao trait `CacheTier`**

Em `src/cache/mod.rs`, no `pub trait CacheTier`, adicionar após `set`:

```rust
    async fn invalidate(&self, id: u64) -> Result<(), TierError>;
```

- [ ] **Step 4: Implementar `Cache::invalidate`**

Em `src/cache/mod.rs`, no `impl Cache`, adicionar:

```rust
    /// Remove um id do cache: L1 sempre; L2 best-effort (breaker + timeout), como
    /// as demais ops de tier. Usado quando um link é editado ou apagado, pra o
    /// redirect parar de servir o valor velho.
    pub async fn invalidate(&self, id: u64) {
        self.hot.invalidate(&id);
        if let Some(l2) = &self.l2 {
            let n = now();
            if self.breaker.allow(n) {
                match tokio::time::timeout(L2_OP_TIMEOUT, l2.invalidate(id)).await {
                    Ok(Ok(())) => self.breaker.record_success(),
                    Ok(Err(_)) | Err(_) => self.breaker.record_failure(n),
                }
            }
        }
    }
```

- [ ] **Step 5: Implementar nos fakes de teste + no ValkeyTier**

Em `src/cache/mod.rs`, os tiers fake do módulo de testes (`FailingTier`, `HangingTier`) precisam do método novo:

```rust
        // dentro de impl CacheTier for FailingTier
        async fn invalidate(&self, _id: u64) -> Result<(), TierError> {
            Err(TierError("down".into()))
        }
```
```rust
        // dentro de impl CacheTier for HangingTier
        async fn invalidate(&self, _id: u64) -> Result<(), TierError> {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            Ok(())
        }
```

Em `src/cache/valkey.rs`, no `impl CacheTier for ValkeyTier`, adicionar:

```rust
    async fn invalidate(&self, id: u64) -> Result<(), TierError> {
        let mut conn = self.conn.clone();
        conn.del::<_, ()>(Self::key(id))
            .await
            .map_err(|e| TierError(e.to_string()))?;
        Ok(())
    }
```

- [ ] **Step 6: Rodar e ver passar**

Run: `cargo test --lib cache`
Expected: PASS (todos os testes de cache, incluindo `invalidate_remove_do_l1`).

- [ ] **Step 7: fmt/clippy + commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/cache/
git commit -m "feat(cache): Cache::invalidate(id) (L1 + L2 best-effort) + CacheTier::invalidate"
```

---

## Task 3: `GET /admin/links` (lista keyset paginada)

**Files:**
- Modify: `src/api.rs` (handler `admin_links_list`, `ListParams`, `LinkRow`, rota)
- Test: `tests/api_it.rs`

**Interfaces:**
- Consumes: `Store::list_links`/`list_aliases` (Task 1); `admin_guard`, `codec::to_base62`, `permute::encode` (existentes).
- Produces: rota `GET /admin/links`.

- [ ] **Step 1: Teste de integração que falha**

Em `tests/api_it.rs`, adicionar (usando `app_admin` que já existe do Tijolo 7):

```rust
#[tokio::test]
async fn admin_links_lista_paginada() {
    let app = app_admin("segredo").await;
    // cria 2 links
    for u in ["https://a.com", "https://b.com"] {
        app.clone()
            .oneshot(
                Request::post("/")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"url":"{u}"}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
    }
    // lista
    let resp = app
        .oneshot(
            Request::get("/admin/links?limit=10")
                .header("x-admin-token", "segredo")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let links = v["links"].as_array().unwrap();
    assert_eq!(links.len(), 2);
    assert!(links[0]["code"].as_str().unwrap().len() == 7);
    assert_eq!(links[0]["url"], "https://a.com");
}

#[tokio::test]
async fn admin_links_sem_token_404() {
    let app = app().await; // admin_token: None
    let resp = app
        .oneshot(Request::get("/admin/links").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
```

- [ ] **Step 2: Rodar e ver falhar**

Run: `cargo test --test api_it admin_links`
Expected: FAIL (rota não existe → 404 onde se espera 200; ou compilação).

- [ ] **Step 3: Implementar o handler + tipos**

Em `src/api.rs`, adicionar os imports que faltarem (`use axum::extract::Query;`) e, junto aos outros handlers admin:

```rust
#[derive(serde::Deserialize)]
struct ListParams {
    after: Option<u64>,
    limit: Option<usize>,
}

#[derive(Serialize)]
struct LinkRow {
    id: u64,
    code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    alias: Option<String>,
    url: String,
    expiry: Option<u64>,
    created: u64,
}

async fn admin_links_list(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(p): Query<ListParams>,
) -> Response {
    if let Err(status) = admin_guard(&st, &headers) {
        return status.into_response();
    }
    let limit = p.limit.unwrap_or(50).clamp(1, 500);
    let links = match st.store.list_links(p.after, limit).await {
        Ok(l) => l,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    // mapa id -> alias (um único list_aliases por request)
    let alias_map: std::collections::HashMap<u64, String> = match st.store.list_aliases().await {
        Ok(pairs) => pairs.into_iter().map(|(a, id)| (id, a)).collect(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let next_after = links.last().map(|(id, _)| *id);
    let rows: Vec<LinkRow> = links
        .into_iter()
        .map(|(id, rec)| LinkRow {
            id,
            code: codec::to_base62(permute::encode(id, st.key)),
            alias: alias_map.get(&id).cloned(),
            url: rec.url,
            expiry: rec.expiry,
            created: rec.created,
        })
        .collect();
    Json(serde_json::json!({ "links": rows, "next_after": next_after })).into_response()
}
```

- [ ] **Step 4: Registrar a rota**

Em `src/api.rs`, na função `router` (ou `router_with_cors` após a Task 5), adicionar ao `Router::new()`:

```rust
        .route("/admin/links", get(admin_links_list))
```

- [ ] **Step 5: Rodar e ver passar**

Run: `cargo test --test api_it admin_links && cargo test --lib`
Expected: PASS.

- [ ] **Step 6: fmt/clippy + commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/api.rs tests/api_it.rs
git commit -m "feat(api): GET /admin/links — lista keyset paginada (code recomputado, alias via 1 list_aliases)"
```

---

## Task 4: `DELETE` e `PATCH /admin/links/:code`

**Files:**
- Modify: `src/api.rs` (handlers + rota)
- Test: `tests/api_it.rs`

**Interfaces:**
- Consumes: `Store::delete_link`/`delete_alias`/`get_link`/`get_alias`/`put_link` (Task 1 + existentes); `Cache::invalidate` (Task 2); `codec::from_base62`, `permute`, `now` (existentes).
- Produces: rotas `DELETE` e `PATCH /admin/links/:code`.

- [ ] **Step 1: Testes que falham**

Em `tests/api_it.rs`:

```rust
async fn cria_e_pega_code(app: &axum::Router, url: &str) -> String {
    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"url":"{url}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    v["code"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn admin_delete_link_vira_404_no_redirect() {
    let app = app_admin("segredo").await;
    let code = cria_e_pega_code(&app, "https://del.com").await;
    // antes: redireciona
    let r = app.clone().oneshot(Request::get(format!("/{code}")).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(r.status(), StatusCode::FOUND);
    // delete
    let r = app.clone().oneshot(
        Request::delete(format!("/admin/links/{code}")).header("x-admin-token", "segredo").body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    // depois: 404
    let r = app.oneshot(Request::get(format!("/{code}")).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn admin_patch_link_atualiza_destino() {
    let app = app_admin("segredo").await;
    let code = cria_e_pega_code(&app, "https://velho.com").await;
    let r = app.clone().oneshot(
        Request::patch(format!("/admin/links/{code}"))
            .header("x-admin-token", "segredo")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"url":"https://novo.com"}"#)).unwrap()
    ).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let r = app.oneshot(Request::get(format!("/{code}")).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(r.status(), StatusCode::FOUND);
    assert_eq!(r.headers()["location"], "https://novo.com");
}

#[tokio::test]
async fn admin_delete_inexistente_404() {
    let app = app_admin("segredo").await;
    let r = app.oneshot(
        Request::delete("/admin/links/0000000").header("x-admin-token", "segredo").body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}
```

- [ ] **Step 2: Rodar e ver falhar**

Run: `cargo test --test api_it admin_delete admin_patch`
Expected: FAIL (rotas não existem).

- [ ] **Step 3: Implementar os handlers**

Em `src/api.rs`, adicionar. Um helper resolve o code em (id, alias-a-remover):

```rust
/// Resolve o code em (id, alias_opcional). Se o code é numérico, não há alias
/// a remover; se é uma string de alias, devolve o alias pra apagar junto.
async fn resolve_for_admin(st: &AppState, code: &str) -> Result<Option<(u64, Option<String>)>, StoreError> {
    match codec::from_base62(code) {
        Some(c) if c <= permute::MAX_ID => Ok(Some((permute::decode(c, st.key), None))),
        _ => match st.store.get_alias(code).await? {
            Some(id) => Ok(Some((id, Some(code.to_string())))),
            None => Ok(None),
        },
    }
}

async fn admin_link_delete(
    State(st): State<Arc<AppState>>,
    Path(code): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(status) = admin_guard(&st, &headers) {
        return status.into_response();
    }
    let (id, alias) = match resolve_for_admin(&st, &code).await {
        Ok(Some(v)) => v,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    // 404 se o link em si não existe
    match st.store.get_link(id).await {
        Ok(Some(_)) => {}
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
    if st.store.delete_link(id).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if let Some(a) = alias {
        let _ = st.store.delete_alias(&a).await;
    }
    st.cache.invalidate(id).await;
    StatusCode::OK.into_response()
}

async fn admin_link_patch(
    State(st): State<Arc<AppState>>,
    Path(code): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(status) = admin_guard(&st, &headers) {
        return status.into_response();
    }
    let (id, _) = match resolve_for_admin(&st, &code).await {
        Ok(Some(v)) => v,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let mut rec = match st.store.get_link(id).await {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    // corpo: chaves ausentes = mantém; "url" atualiza; "ttl": número recomputa
    // expiry (now+ttl), null remove a expiração.
    let patch: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return (StatusCode::BAD_REQUEST, "json inválido").into_response(),
    };
    if let Some(u) = patch.get("url") {
        match u.as_str() {
            Some(s) if is_valid_url(s) => rec.url = s.to_string(),
            _ => return (StatusCode::BAD_REQUEST, "url inválida").into_response(),
        }
    }
    if let Some(ttl) = patch.get("ttl") {
        if ttl.is_null() {
            rec.expiry = None;
        } else if let Some(secs) = ttl.as_u64() {
            match now().checked_add(secs) {
                Some(e) => rec.expiry = Some(e),
                None => return (StatusCode::BAD_REQUEST, "ttl inválido").into_response(),
            }
        } else {
            return (StatusCode::BAD_REQUEST, "ttl inválido").into_response();
        }
    }
    if st.store.put_link(id, &rec).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    st.cache.invalidate(id).await;
    StatusCode::OK.into_response()
}
```

(`Bytes` já é importado desde o Tijolo 7; se não estiver, adicionar `use axum::body::Bytes;`.)

- [ ] **Step 4: Registrar as rotas**

Em `src/api.rs`, no `Router::new()`:

```rust
        .route(
            "/admin/links/:code",
            axum::routing::delete(admin_link_delete).patch(admin_link_patch),
        )
```

- [ ] **Step 5: Rodar e ver passar**

Run: `cargo test --test api_it && cargo test --lib`
Expected: PASS (todos, incluindo delete/patch e os antigos).

- [ ] **Step 6: fmt/clippy + commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/api.rs tests/api_it.rs
git commit -m "feat(api): DELETE e PATCH /admin/links/:code (apaga link+alias, edita url/ttl, invalida cache)"
```

---

## Task 5: CORS opt-in (`QUARK_CORS_ORIGINS`)

**Files:**
- Modify: `Cargo.toml` (dependência `tower-http`)
- Modify: `src/api.rs` (`parse_cors_origins`, `router_with_cors`, `router` delega)
- Test: `src/api.rs` (unit da parse) + `tests/api_it.rs` (header presente)

**Interfaces:**
- Produces:
  - `fn parse_cors_origins(raw: Option<String>) -> Vec<String>` — split por vírgula, trim, descarta vazios.
  - `pub fn router_with_cors(state: Arc<AppState>, origins: Vec<String>) -> Router` — monta o router e aplica `CorsLayer` se `origins` não vazio. `router(state)` passa a delegar com `parse_cors_origins(std::env::var("QUARK_CORS_ORIGINS").ok())`.

- [ ] **Step 1: Adicionar a dependência**

Em `Cargo.toml`, na seção `[dependencies]`, adicionar:

```toml
tower-http = { version = "0.6", features = ["cors"] }
```

- [ ] **Step 2: Testes que falham**

Em `src/api.rs`, no `#[cfg(test)] mod tests`, adicionar:

```rust
    use super::parse_cors_origins;

    #[test]
    fn parse_cors_origins_split_e_trim() {
        assert_eq!(parse_cors_origins(None), Vec::<String>::new());
        assert_eq!(parse_cors_origins(Some("".into())), Vec::<String>::new());
        assert_eq!(
            parse_cors_origins(Some(" https://a.com , https://b.com ".into())),
            vec!["https://a.com".to_string(), "https://b.com".to_string()]
        );
    }
```

Em `tests/api_it.rs`:

```rust
#[tokio::test]
async fn cors_header_presente_quando_configurado() {
    // monta o router com uma origem permitida explícita (sem env)
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path()).await.unwrap();
    let cache = Cache::new(store.clone(), 1000);
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
    let store2 = store.clone();
    let state = Arc::new(AppState {
        cache, store, key: 0x1234, analytics_tx: tx, sink, admin_token: None,
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        blocklist: quark::abuse::blocklist::Blocklist::new(store2, None, 60),
        block_private: true, public_host: None, real_ip_header: "cf-connecting-ip".into(),
    });
    let app = quark::api::router_with_cors(state, vec!["https://painel.example".into()]);
    let resp = app
        .oneshot(
            Request::get("/health")
                .header("origin", "https://painel.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.headers().get("access-control-allow-origin").unwrap(),
        "https://painel.example"
    );
}
```

- [ ] **Step 3: Rodar e ver falhar**

Run: `cargo test --lib api::tests::parse_cors && cargo test --test api_it cors_header`
Expected: FAIL (funções não existem).

- [ ] **Step 4: Implementar**

Em `src/api.rs`, adicionar os imports (`use tower_http::cors::{Any, CorsLayer}; use axum::http::Method;`) e:

```rust
/// Origens de CORS a partir da env `QUARK_CORS_ORIGINS` (lista por vírgula).
pub fn parse_cors_origins(raw: Option<String>) -> Vec<String> {
    match raw {
        None => Vec::new(),
        Some(s) => s
            .split(',')
            .map(|o| o.trim().to_string())
            .filter(|o| !o.is_empty())
            .collect(),
    }
}
```

Trocar a função `router` atual por uma delegação + `router_with_cors`:

```rust
pub fn router(state: Arc<AppState>) -> Router {
    let origins = parse_cors_origins(std::env::var("QUARK_CORS_ORIGINS").ok());
    router_with_cors(state, origins)
}

pub fn router_with_cors(state: Arc<AppState>, origins: Vec<String>) -> Router {
    let app = Router::new()
        .route("/", post(create))
        .route("/health", get(health))
        .route("/:code", get(redirect))
        .route("/:code/stats", get(stats))
        .route(
            "/admin/blocklist",
            get(blocklist_get).post(blocklist_add).delete(blocklist_delete),
        )
        .route("/admin/links", get(admin_links_list))
        .route(
            "/admin/links/:code",
            axum::routing::delete(admin_link_delete).patch(admin_link_patch),
        )
        .with_state(state);

    let app = if origins.is_empty() {
        app
    } else {
        let list: Vec<axum::http::HeaderValue> =
            origins.iter().filter_map(|o| o.parse().ok()).collect();
        let cors = CorsLayer::new()
            .allow_origin(list)
            .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::DELETE])
            .allow_headers(Any);
        app.layer(cors)
    };

    // Log de acesso por request é opt-in (mantido do estado atual).
    if std::env::var("QUARK_ACCESS_LOG").is_ok() {
        app.layer(axum::middleware::from_fn(log_requests))
    } else {
        app
    }
}
```

(Isto substitui o corpo atual de `router`, que já montava as rotas + o access-log. As rotas de `/admin/links*` do Task 3/4 passam a viver aqui.)

- [ ] **Step 5: Rodar e ver passar**

Run: `cargo test --lib && cargo test --test api_it`
Expected: PASS (parse unit + cors header + tudo que já passava).

- [ ] **Step 6: fmt/clippy + commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add Cargo.toml Cargo.lock src/api.rs tests/api_it.rs
git commit -m "feat(api): CORS opt-in via QUARK_CORS_ORIGINS (router_with_cors + tower-http)"
```

---

## Task 6: `docker-compose.yml` + docs

**Files:**
- Create: `docker-compose.yml`
- Modify: `README.md`

**Interfaces:** nenhuma (infra/docs).

- [ ] **Step 1: Criar `docker-compose.yml`**

Na raiz do repo:

```yaml
# Stack completa pra dev local / referência de self-host full-stack.
# `docker compose up --build` sobe quark + os 3 backends plugáveis.
services:
  postgres:
    image: postgres:16
    environment:
      POSTGRES_PASSWORD: quark
      POSTGRES_USER: quark
      POSTGRES_DB: quark
    ports: ["5432:5432"]
    healthcheck:
      test: ["CMD", "pg_isready", "-U", "quark"]
      interval: 5s
      timeout: 3s
      retries: 5

  valkey:
    image: valkey/valkey:8
    ports: ["6379:6379"]
    healthcheck:
      test: ["CMD", "valkey-cli", "ping"]
      interval: 5s
      timeout: 3s
      retries: 5

  clickhouse:
    image: clickhouse/clickhouse-server:24
    ports: ["8123:8123"]
    ulimits:
      nofile: { soft: 262144, hard: 262144 }

  quark:
    build: .
    depends_on:
      postgres: { condition: service_healthy }
      valkey: { condition: service_healthy }
    ports: ["8080:8080"]
    environment:
      QUARK_KEY: "12345678901234567"        # dev only — troque em produção
      QUARK_ADMIN_TOKEN: "dev-admin-token"
      QUARK_DATABASE_URL: "postgres://quark:quark@postgres:5432/quark"
      QUARK_VALKEY_URL: "redis://valkey:6379"
      QUARK_CLICKHOUSE_URL: "http://clickhouse:8123"
      QUARK_CORS_ORIGINS: "http://localhost:5173"   # origem do painel em dev
```

- [ ] **Step 2: Documentar no README**

Em `README.md`, adicionar uma subseção na seção **Operating** (ou perto do `docs/DEPLOY.md`), no mesmo estilo:

Run: `grep -n "## Operating\|docker" README.md`

Adicionar:
```markdown
### Local dev stack

`docker compose up --build` brings up quark plus all three optional backends
(Postgres, Valkey, ClickHouse) wired together — handy for development, for
running the gated integration tests, and as a full-stack self-host reference.
The admin/panel API lives under `/admin/*` (token `QUARK_ADMIN_TOKEN`): list
links `GET /admin/links`, delete `DELETE /admin/links/:code`, edit
`PATCH /admin/links/:code`. A separate web panel (SPA) consumes this API; set
`QUARK_CORS_ORIGINS` to the panel's origin.
```

E acrescentar as envs novas na tabela de configuração (mesmo formato das existentes):
```markdown
| `QUARK_CORS_ORIGINS` | Comma-separated origins allowed to call the API (for the web panel). | unset → no CORS (same-origin only) |
```

- [ ] **Step 3: Validar o compose (sintaxe) + commit**

Run: `docker compose config >/dev/null && echo OK` (valida a sintaxe do YAML sem subir nada; pular se não houver docker no ambiente do executor).
Expected: `OK` (ou pular).

```bash
git add docker-compose.yml README.md
git commit -m "chore: docker-compose full-stack (dev/self-host) + docs do painel API e QUARK_CORS_ORIGINS"
```

---

## Self-Review (preenchido pelo autor do plano)

**Cobertura da spec:**
- `GET /admin/links` keyset + code + alias(1x) sem clicks → Task 3 (+ Task 1 `list_links`/`list_aliases`). ✓
- `DELETE /admin/links/:code` (+ alias) → Task 4 (+ Task 1 `delete_*`). ✓
- `PATCH` url/ttl (+ null limpa expiry) + invalida cache → Task 4 (+ Task 2 `invalidate`). ✓
- CORS via `QUARK_CORS_ORIGINS` → Task 5. ✓
- Auth reusa `QUARK_ADMIN_TOKEN`/`admin_guard` → Tasks 3/4 (sem AppState novo). ✓
- docker-compose + docs → Task 6. ✓
- Redirect/leitura intocados → confirmado (só rotas `/admin/*` novas + CORS). ✓

**Placeholders:** nenhum — todo passo traz o código completo.

**Consistência de tipos:** `list_links(Option<u64>, usize) -> Vec<(u64, Record)>`, `list_aliases() -> Vec<(String,u64)>`, `delete_link(u64)`, `delete_alias(&str)`, `Cache::invalidate(u64)`, `CacheTier::invalidate(u64) -> Result<(),TierError>`, `parse_cors_origins(Option<String>) -> Vec<String>`, `router_with_cors(Arc<AppState>, Vec<String>) -> Router` — idênticos entre tasks. Nenhum campo novo em `AppState` (os 5 construtores de teste não quebram).

**Nota de escopo:** `delete_link` não remove stats/events de analytics do id (órfãos inofensivos; limpeza fica pra depois se necessário) — decisão consciente, fora do escopo deste tijolo.
