# Tijolo 6 — Escala Horizontal (plano de implementação)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Permitir rodar N réplicas do quark sobre um Postgres compartilhado sem colisão de IDs, e blindar o LMDB contra colisão silenciosa de código via um `QUARK_NODE_ID` defensivo opt-in.

**Architecture:** O caminho de escala é storage compartilhado (Postgres): a sequência global já garante IDs únicos entre réplicas — o trabalho é provar isso com testes de integração. O LMDB (single-node por natureza) ganha um particionamento opt-in do espaço de 40 bits: `QUARK_NODE_ID` (0–255) ocupa os 8 bits altos, o contador local ocupa os 32 baixos; ausente = comportamento atual intacto.

**Tech Stack:** Rust 2021, heed (LMDB), sqlx (Postgres), axum/tower (oneshot nos testes), serial_test, tokio, Mermaid nos docs.

## Global Constraints

- Divisão de bits fixa: **8 bits de nó (altos) + 32 bits de contador local**. `NODE_BITS=8`, `LOCAL_BITS=32`, `LOCAL_MAX = 2^32 - 1 = 4_294_967_295`.
- `QUARK_NODE_ID` **ausente** = comportamento atual preservado (contador usa os 40 bits inteiros; capacidade `permute::MAX_ID` = 2^40 − 1). Zero mudança para o single-node.
- `QUARK_NODE_ID` **definido** deve estar em 0–255 (validação via `u8`); fora disso → erro no startup (fail-fast).
- Invariante de não-vazamento: no modo com node-id, quando o contador local passaria de `LOCAL_MAX`, `next_id` retorna erro **antes** de compor o ID e **antes** de gravar (o ID nunca vaza para o prefixo do vizinho).
- Estouro do espaço de id → `INSUFFICIENT_STORAGE` (507) no handler `create`, igual ao tratamento já existente de `id > MAX_ID`.
- Nenhuma mudança em `src/permute.rs` (a Feistel é indiferente a como o ID foi alocado).
- Testes de Postgres são **gated** por `QUARK_TEST_DATABASE_URL`; sem a env, pulam (retornam cedo). Devem sempre **compilar**.
- Documentação a nível humano, com Mermaid válido.

---

## File Structure

- `src/store/mod.rs` — adiciona a variante `StoreError::IdSpaceExhausted` (+ braço no `Display`).
- `src/store/lmdb.rs` — constantes `NODE_BITS/LOCAL_BITS/LOCAL_MAX`; helpers puros `parse_node_id` e `compose_id`; campo `node_id: Option<u8>` no `LmdbStore`; construtor `open_with_node_id`; `open` passa a ler `QUARK_NODE_ID`; `next_id` usa `compose_id`. Testes unitários in-module.
- `src/api.rs` — mapeia `StoreError::IdSpaceExhausted` → `507` nos dois pontos de `next_id` dentro de `create`.
- `src/main.rs` — log informativo do node-id no startup.
- `tests/horizontal_scale_it.rs` (novo) — testes de integração Postgres multi-réplica (gated).
- `docs/SCALING.md` (novo) — os 3 formatos, Mermaid, tabela de capacidade, regra tudo-ou-nada, limites honestos.
- `README.md` — uma linha linkando `docs/SCALING.md`.

---

## Task 1: Helpers puros de particionamento + variante de erro

**Files:**
- Modify: `src/store/mod.rs` (enum `StoreError` e seu `Display`)
- Modify: `src/store/lmdb.rs` (constantes + `parse_node_id` + `compose_id` + testes in-module)

**Interfaces:**
- Consumes: `permute::MAX_ID` (só conceitualmente; não referenciado aqui).
- Produces:
  - `StoreError::IdSpaceExhausted` (variante unitária).
  - `const NODE_BITS: u32 = 8;`, `const LOCAL_BITS: u32 = 32;`, `const LOCAL_MAX: u64 = (1u64 << LOCAL_BITS) - 1;` (privados em `lmdb.rs`).
  - `fn parse_node_id(raw: Option<String>) -> Result<Option<u8>, StoreError>` (privado).
  - `fn compose_id(node_id: Option<u8>, counter: u64) -> Result<u64, StoreError>` (privado).

- [ ] **Step 1: Adicionar a variante `IdSpaceExhausted`**

Em `src/store/mod.rs`, no enum:

```rust
#[derive(Debug)]
pub enum StoreError {
    Db(heed::Error),
    Serde(serde_json::Error),
    Backend(String),
    IdSpaceExhausted,
}
```

E no `Display` (adicionar o braço):

```rust
impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Db(e) => write!(f, "db: {e}"),
            StoreError::Serde(e) => write!(f, "serde: {e}"),
            StoreError::Backend(s) => write!(f, "backend: {s}"),
            StoreError::IdSpaceExhausted => write!(f, "espaço de id esgotado"),
        }
    }
}
```

- [ ] **Step 2: Escrever os testes que falham (helpers puros)**

Em `src/store/lmdb.rs`, adicionar no fim do arquivo um módulo de testes (não existe hoje):

```rust
#[cfg(test)]
mod tests {
    use super::{compose_id, parse_node_id, LOCAL_BITS, LOCAL_MAX};
    use crate::store::StoreError;

    #[test]
    fn parse_node_id_ausente_ou_vazio_vira_none() {
        assert!(matches!(parse_node_id(None), Ok(None)));
        assert!(matches!(parse_node_id(Some(String::new())), Ok(None)));
    }

    #[test]
    fn parse_node_id_valido() {
        assert!(matches!(parse_node_id(Some("0".into())), Ok(Some(0))));
        assert!(matches!(parse_node_id(Some("255".into())), Ok(Some(255))));
        assert!(matches!(parse_node_id(Some("7".into())), Ok(Some(7))));
    }

    #[test]
    fn parse_node_id_invalido_erra() {
        assert!(matches!(parse_node_id(Some("256".into())), Err(StoreError::Backend(_))));
        assert!(matches!(parse_node_id(Some("-1".into())), Err(StoreError::Backend(_))));
        assert!(matches!(parse_node_id(Some("abc".into())), Err(StoreError::Backend(_))));
    }

    #[test]
    fn compose_id_sem_node_e_identidade() {
        // modo default: contador vira o id direto (checagem de MAX_ID fica no chamador)
        assert_eq!(compose_id(None, 1).unwrap(), 1);
        assert_eq!(compose_id(None, 1_000_000_000).unwrap(), 1_000_000_000);
    }

    #[test]
    fn compose_id_com_node_prefixa_os_bits_altos() {
        assert_eq!(compose_id(Some(0), 1).unwrap(), 1);
        assert_eq!(compose_id(Some(1), 1).unwrap(), (1u64 << LOCAL_BITS) | 1);
        assert_eq!(compose_id(Some(5), 42).unwrap(), (5u64 << LOCAL_BITS) | 42);
    }

    #[test]
    fn compose_id_faixas_de_nos_sao_disjuntas() {
        // o maior id do nó 0 é menor que o menor id do nó 1
        let maior_no_0 = compose_id(Some(0), LOCAL_MAX).unwrap();
        let menor_no_1 = compose_id(Some(1), 1).unwrap();
        assert!(maior_no_0 < menor_no_1);
    }

    #[test]
    fn compose_id_estouro_do_contador_local_erra() {
        assert_eq!(compose_id(Some(3), LOCAL_MAX).unwrap(), (3u64 << LOCAL_BITS) | LOCAL_MAX);
        assert!(matches!(compose_id(Some(3), LOCAL_MAX + 1), Err(StoreError::IdSpaceExhausted)));
    }
}
```

- [ ] **Step 3: Rodar os testes e ver falhar**

Run: `cargo test --lib store::lmdb::tests`
Expected: FAIL na compilação — `compose_id`, `parse_node_id`, `LOCAL_BITS`, `LOCAL_MAX` não existem ainda.

- [ ] **Step 4: Implementar constantes e helpers**

Em `src/store/lmdb.rs`, logo após os `use` do topo (antes de `pub struct LmdbStore`):

```rust
/// Particionamento defensivo do espaço de 40 bits entre nós (ver docs/SCALING.md).
/// Os 8 bits altos identificam o nó; os 32 baixos são o contador local do nó.
const NODE_BITS: u32 = 8;
const LOCAL_BITS: u32 = 40 - NODE_BITS; // 32
const LOCAL_MAX: u64 = (1u64 << LOCAL_BITS) - 1; // 4_294_967_295

/// Lê `QUARK_NODE_ID`: ausente/vazio → `None` (modo single-node, 40 bits inteiros);
/// "0".."255" → `Some(n)`; qualquer outra coisa → erro (fail-fast no startup).
fn parse_node_id(raw: Option<String>) -> Result<Option<u8>, StoreError> {
    match raw.as_deref() {
        None | Some("") => Ok(None),
        Some(s) => s
            .parse::<u8>()
            .map(Some)
            .map_err(|_| StoreError::Backend(format!("QUARK_NODE_ID inválido: {s} (esperado 0-255)"))),
    }
}

/// Compõe o id final a partir do contador. Sem node-id: identidade (o chamador
/// aplica o teto de `permute::MAX_ID`, como hoje). Com node-id: prefixa os bits
/// altos e falha ANTES de estourar a faixa local (invariante de não-vazamento).
fn compose_id(node_id: Option<u8>, counter: u64) -> Result<u64, StoreError> {
    match node_id {
        None => Ok(counter),
        Some(node) => {
            if counter > LOCAL_MAX {
                Err(StoreError::IdSpaceExhausted)
            } else {
                Ok(((node as u64) << LOCAL_BITS) | counter)
            }
        }
    }
}
```

- [ ] **Step 5: Rodar os testes e ver passar**

Run: `cargo test --lib store::lmdb::tests`
Expected: PASS (6 testes).

- [ ] **Step 6: fmt + clippy**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings`
Expected: sem alterações do fmt, clippy limpo.

- [ ] **Step 7: Commit**

```bash
git add src/store/mod.rs src/store/lmdb.rs
git commit -m "feat(store): helpers de particionamento de id (node-prefix) + StoreError::IdSpaceExhausted"
```

---

## Task 2: Ligar o node-id no LmdbStore, na API e no startup

**Files:**
- Modify: `src/store/lmdb.rs` (campo `node_id`, `open_with_node_id`, `open`, `next_id`, mais testes in-module)
- Modify: `src/api.rs` (função `create`: mapear `IdSpaceExhausted` → 507 nos dois `next_id`)
- Modify: `src/main.rs` (log de node-id no startup)

**Interfaces:**
- Consumes: `parse_node_id`, `compose_id` (Task 1); `StoreError::IdSpaceExhausted` (Task 1).
- Produces: `LmdbStore::open_with_node_id(path: &Path, node_id: Option<u8>) -> Result<LmdbStore, StoreError>` (pub). `LmdbStore::open(path)` mantém a mesma assinatura.

- [ ] **Step 1: Escrever os testes que falham (comportamento do store)**

Adicionar ao `#[cfg(test)] mod tests` de `src/store/lmdb.rs` (criado na Task 1):

```rust
    use super::LmdbStore;
    use crate::store::Store;

    #[tokio::test]
    async fn next_id_default_e_compativel_com_hoje() {
        let dir = tempfile::tempdir().unwrap();
        // sem node-id: ids 1, 2, 3... nos 40 bits inteiros (igual ao comportamento atual)
        let s = LmdbStore::open_with_node_id(dir.path(), None).unwrap();
        assert_eq!(s.next_id().await.unwrap(), 1);
        assert_eq!(s.next_id().await.unwrap(), 2);
        assert_eq!(s.next_id().await.unwrap(), 3);
    }

    #[tokio::test]
    async fn next_id_com_node_prefixa_e_incrementa_local() {
        let dir = tempfile::tempdir().unwrap();
        let s = LmdbStore::open_with_node_id(dir.path(), Some(5)).unwrap();
        assert_eq!(s.next_id().await.unwrap(), (5u64 << LOCAL_BITS) | 1);
        assert_eq!(s.next_id().await.unwrap(), (5u64 << LOCAL_BITS) | 2);
    }

    #[tokio::test]
    async fn next_id_de_nos_distintos_nao_colide() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let a = LmdbStore::open_with_node_id(dir_a.path(), Some(0)).unwrap();
        let b = LmdbStore::open_with_node_id(dir_b.path(), Some(1)).unwrap();
        // mesmo contador local (1) em nós diferentes → ids diferentes
        assert_ne!(a.next_id().await.unwrap(), b.next_id().await.unwrap());
    }
```

- [ ] **Step 2: Rodar e ver falhar**

Run: `cargo test --lib store::lmdb::tests`
Expected: FAIL na compilação — `open_with_node_id` não existe.

- [ ] **Step 3: Adicionar o campo `node_id` e os construtores**

Em `src/store/lmdb.rs`, adicionar o campo na struct:

```rust
pub struct LmdbStore {
    env: Env,
    links: Database<BeU64, Bytes>,  // id -> Record (json)
    aliases: Database<Str, BeU64>,  // alias -> id
    meta: Database<Str, BeU64>,     // "next_id" -> u64 (contador local; nos 40 bits no modo default)
    stats: Database<BeU64, Bytes>,  // id -> Aggregates (json)
    events: Database<BeU64, Bytes>, // id -> Vec<ClickEvent> (json, truncado a EVENTS_MAX)
    node_id: Option<u8>,            // Some => particiona o espaço de id; None => 40 bits inteiros
}
```

Substituir o `impl LmdbStore { pub fn open... }` por:

```rust
impl LmdbStore {
    /// Abre lendo `QUARK_NODE_ID` do ambiente (fail-fast se inválido).
    pub fn open(path: &Path) -> Result<LmdbStore, StoreError> {
        let node_id = parse_node_id(std::env::var("QUARK_NODE_ID").ok())?;
        LmdbStore::open_with_node_id(path, node_id)
    }

    /// Abre com um node-id explícito (usado por testes; evita corrida de env global).
    pub fn open_with_node_id(path: &Path, node_id: Option<u8>) -> Result<LmdbStore, StoreError> {
        std::fs::create_dir_all(path).map_err(heed::Error::Io)?;
        let env = unsafe {
            EnvOpenOptions::new()
                .map_size(64 * 1024 * 1024 * 1024) // 64 GiB de espaço de endereço virtual (mmap)
                .max_dbs(5)
                .open(path)?
        };
        let mut wtxn = env.write_txn()?;
        let links = env.create_database(&mut wtxn, Some("links"))?;
        let aliases = env.create_database(&mut wtxn, Some("aliases"))?;
        let meta = env.create_database(&mut wtxn, Some("meta"))?;
        let stats = env.create_database(&mut wtxn, Some("stats"))?;
        let events = env.create_database(&mut wtxn, Some("events"))?;
        wtxn.commit()?;
        Ok(LmdbStore {
            env,
            links,
            aliases,
            meta,
            stats,
            events,
            node_id,
        })
    }
}
```

- [ ] **Step 4: Usar `compose_id` no `next_id`**

Em `src/store/lmdb.rs`, substituir o corpo do `next_id`:

```rust
    async fn next_id(&self) -> Result<u64, StoreError> {
        let mut wtxn = self.env.write_txn()?;
        let cur = self.meta.get(&wtxn, "next_id")?.unwrap_or(0);
        let next = cur + 1;
        // Compõe ANTES de gravar: se o contador local estourou a faixa do nó,
        // o erro sai aqui e o contador não avança (não vaza pro prefixo vizinho).
        let id = compose_id(self.node_id, next)?;
        self.meta.put(&mut wtxn, "next_id", &next)?;
        wtxn.commit()?;
        Ok(id)
    }
```

- [ ] **Step 5: Rodar e ver passar**

Run: `cargo test --lib store::lmdb::tests`
Expected: PASS (9 testes no total do módulo).

- [ ] **Step 6: Mapear `IdSpaceExhausted` → 507 na API**

Em `src/api.rs`, dentro de `create`, existem **dois** `match st.store.next_id().await` (o do caminho de alias e o do caminho sem alias). Em **ambos**, adicionar o braço de exaustão. A forma final de cada um:

```rust
        let id = match st.store.next_id().await {
            Ok(id) => id,
            Err(StoreError::IdSpaceExhausted) => {
                return (StatusCode::INSUFFICIENT_STORAGE, "espaço de id esgotado").into_response()
            }
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        };
```

(`StoreError` já está importado em `src/api.rs`. A checagem existente `if id > permute::MAX_ID` no caminho sem alias permanece — cobre a exaustão do modo default.)

- [ ] **Step 7: Log de node-id no startup**

Em `src/main.rs`, logo após o bloco que loga `analytics sink:` (antes da construção do `cache`), adicionar:

```rust
    match std::env::var("QUARK_NODE_ID") {
        Ok(n) if std::env::var("QUARK_DATABASE_URL").is_ok() => {
            eprintln!("AVISO: QUARK_NODE_ID={n} ignorado no backend Postgres (node-id é só do LMDB)");
        }
        Ok(n) => eprintln!("node-id LMDB: {n} (espaço de id particionado em 8+32 bits)"),
        Err(_) => {}
    }
```

- [ ] **Step 8: Rodar a suíte inteira de lib + api_it**

Run: `cargo test --lib && cargo test --test api_it`
Expected: PASS em tudo (os testes de `api_it` existentes continuam verdes — o mapeamento novo não muda os caminhos felizes).

- [ ] **Step 9: fmt + clippy**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings`
Expected: fmt sem mudanças, clippy limpo.

- [ ] **Step 10: Commit**

```bash
git add src/store/lmdb.rs src/api.rs src/main.rs
git commit -m "feat(store): QUARK_NODE_ID opt-in particiona o espaço de id no LMDB (default preservado) + 507 na exaustão + log de startup"
```

---

## Task 3: Testes de integração Postgres multi-réplica (gated)

**Files:**
- Create: `tests/horizontal_scale_it.rs`

**Interfaces:**
- Consumes: `quark::store::postgres::PostgresStore`, `quark::store::{Store, AnalyticsSink}` (via `open`), `quark::api::{router, AppState}`, `quark::cache::Cache`, `quark::analytics::AnalyticsSink`.
- Produces: nada (só testes).

- [ ] **Step 1: Escrever o arquivo de testes**

Criar `tests/horizontal_scale_it.rs`:

```rust
//! Escala horizontal: prova que réplicas sobre o mesmo Postgres geram IDs
//! únicos e compartilham dados. Gated por QUARK_TEST_DATABASE_URL; sem a env,
//! os testes pulam (mas compilam sempre).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use quark::analytics::AnalyticsSink;
use quark::api::{router, AppState};
use quark::cache::Cache;
use quark::store::{postgres::PostgresStore, Store};
use serial_test::serial;
use std::collections::HashSet;
use std::sync::Arc;
use tower::ServiceExt; // oneshot

fn test_url() -> Option<String> {
    std::env::var("QUARK_TEST_DATABASE_URL").ok()
}

/// Monta um router quark completo sobre um Postgres já aberto — simula uma réplica.
async fn pg_replica(url: &str) -> axum::Router {
    let pg = Arc::new(PostgresStore::open(url).await.unwrap());
    let store: Arc<dyn Store> = pg.clone();
    let sink: Arc<dyn AnalyticsSink> = pg;
    let cache = Cache::new(store.clone(), 1000);
    let (analytics_tx, _rx) = tokio::sync::mpsc::channel(100);
    let state = Arc::new(AppState {
        cache,
        store,
        key: 0x1234, // mesma key em todas as réplicas (como em produção)
        analytics_tx,
        sink,
        admin_token: None,
    });
    router(state)
}

#[tokio::test]
#[serial(pg)]
async fn ids_unicos_entre_replicas_pg() {
    let Some(url) = test_url() else {
        eprintln!("skip: sem QUARK_TEST_DATABASE_URL");
        return;
    };
    // limpa o estado uma vez
    PostgresStore::open(&url)
        .await
        .unwrap()
        .reset_for_tests()
        .await
        .unwrap();

    // duas "réplicas" = dois stores independentes sobre o mesmo banco
    let a = Arc::new(PostgresStore::open(&url).await.unwrap());
    let b = Arc::new(PostgresStore::open(&url).await.unwrap());

    let mut handles = Vec::new();
    for store in [a.clone(), b.clone()] {
        for _ in 0..200 {
            let st = store.clone();
            handles.push(tokio::spawn(async move { st.next_id().await.unwrap() }));
        }
    }

    let mut ids = HashSet::new();
    for h in handles {
        let id = h.await.unwrap();
        assert!(ids.insert(id), "id duplicado entre réplicas: {id}");
    }
    assert_eq!(ids.len(), 400, "esperava 400 ids únicos");
}

#[tokio::test]
#[serial(pg)]
async fn create_na_replica_a_redirect_na_replica_b_pg() {
    let Some(url) = test_url() else {
        eprintln!("skip: sem QUARK_TEST_DATABASE_URL");
        return;
    };
    PostgresStore::open(&url)
        .await
        .unwrap()
        .reset_for_tests()
        .await
        .unwrap();

    let app_a = pg_replica(&url).await;
    let app_b = pg_replica(&url).await;

    // cria o link na réplica A
    let resp = app_a
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"url":"https://exemplo.com/replica"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let code = v["code"].as_str().unwrap().to_string();

    // resolve o mesmo código na réplica B (cache frio em B → busca no store compartilhado)
    let resp = app_b
        .oneshot(Request::get(format!("/{code}")).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    assert_eq!(resp.headers()["location"], "https://exemplo.com/replica");
}
```

- [ ] **Step 2: Compilar os testes (sem PG rodando, devem pular)**

Run: `cargo test --test horizontal_scale_it`
Expected: compila; ambos os testes imprimem "skip: sem QUARK_TEST_DATABASE_URL" e passam (retorno cedo). Resultado: `2 passed`.

- [ ] **Step 3: (Se houver um Postgres de teste) rodar de verdade**

Run: `QUARK_TEST_DATABASE_URL=postgres://quark:quark@localhost:5432/quark cargo test --test horizontal_scale_it`
Expected: `2 passed` com a lógica real exercida (ids únicos; redirect cross-réplica 302). Sem um PG disponível, pular este passo — o CI já sobe o serviço Postgres.

- [ ] **Step 4: fmt + clippy**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings`
Expected: fmt sem mudanças, clippy limpo.

- [ ] **Step 5: Commit**

```bash
git add tests/horizontal_scale_it.rs
git commit -m "test(scale): unicidade de id multi-réplica + dado compartilhado create-A/redirect-B (Postgres, gated)"
```

---

## Task 4: Documentação — `docs/SCALING.md`

**Files:**
- Create: `docs/SCALING.md`
- Modify: `README.md` (uma linha linkando o novo doc)

**Interfaces:** nenhuma (documentação).

- [ ] **Step 1: Escrever `docs/SCALING.md`**

Criar `docs/SCALING.md` com este conteúdo:

````markdown
# Escala horizontal do quark

O quark escala horizontalmente **compartilhando o storage** entre réplicas. Há
três formatos de deploy, com limites diferentes — escolha pelo que você precisa.

## Os três formatos

```mermaid
flowchart TB
    subgraph F1["1. Binário puro (default)"]
        A1[quark único] --> L1[(LMDB local)]
    end
    subgraph F2["2. Réplicas + Postgres compartilhado (multi-nó recomendado)"]
        LB[Load Balancer] --> R1[quark 1]
        LB --> R2[quark 2]
        LB --> R3[quark N]
        R1 --> PG[(Postgres compartilhado)]
        R2 --> PG
        R3 --> PG
    end
    subgraph F3["3. Vários binários LMDB (não recomendado p/ multi-nó)"]
        N1[quark nó 0] --> D1[(LMDB local 0)]
        N2[quark nó 1] --> D2[(LMDB local 1)]
    end
```

| Formato | Storage | Multi-nó | Observação |
|---|---|---|---|
| **1. Binário puro** | LMDB embutido | Não (1 nó) | Recurso mínimo; capacidade ~1,1 trilhão de links |
| **2. Réplicas + Postgres** | Postgres compartilhado | **Sim** | Caminho recomendado; qualquer réplica serve qualquer link |
| **3. Vários LMDB** | LMDB local por nó | Não p/ leitura | Cada nó só tem os dados que ele criou (ver limites abaixo) |

## Como escalar de verdade (formato 2)

Suba N cópias do binário atrás de um load balancer, todas com a mesma
`QUARK_KEY` e a mesma `QUARK_DATABASE_URL` apontando pro Postgres compartilhado:

- **IDs únicos**: a sequência `quark_id_seq` do Postgres é atômica e cluster-wide;
  réplicas concorrentes nunca geram o mesmo id.
- **Dados compartilhados**: todas leem/escrevem as mesmas tabelas; não há
  afinidade de sessão (o load balancer pode ser round-robin simples).
- **Cache opcional**: um Valkey compartilhado (`QUARK_VALKEY_URL`) como L2 corta
  leituras repetidas no Postgres.

## `QUARK_NODE_ID` — particionamento defensivo do LMDB

O espaço de código do quark tem 40 bits. Quando `QUARK_NODE_ID` está **definido**
(0–255), os 8 bits altos passam a identificar o nó e os 32 baixos são o contador
local daquele nó:

| Bits de nó | Bits locais | Máx. de nós | Links por nó |
|---|---|---|---|
| 8 | 32 | 256 | ~4,3 bilhões |

- **Ausente (default)**: comportamento normal, contador usa os 40 bits inteiros
  (~1,1 trilhão de links). É o modo single-node.
- **Regra tudo-ou-nada**: ou **todos** os nós rodam sem `QUARK_NODE_ID` (= 1 nó),
  ou **todos** rodam com um `QUARK_NODE_ID` **distinto**. Nunca misture um nó sem
  node-id (faixa cheia) com nós particionados — os espaços se sobrepõem.
- `QUARK_NODE_ID` inválido (fora de 0–255) derruba o processo no startup.

## Limite honesto do formato 3

`QUARK_NODE_ID` garante que dois nós LMDB **não gerem o mesmo código** — mas
**não** faz um nó servir os links do outro. Cada LMDB é local: um redirect que
cai no nó errado dá 404, porque aquele nó não tem o dado. Ou seja, o node-id é um
**guard-rail contra colisão**, não um modo multi-nó de verdade.

**Para multi-nó real, use o formato 2 (Postgres compartilhado).** Um modo
shared-nothing completo (com proxy entre nós usando o node-id embutido no código)
é trabalho futuro, não implementado.
````

- [ ] **Step 2: Validar o Mermaid**

Abrir `docs/SCALING.md` e conferir que os dois blocos ```mermaid``` renderizam
(sem erro de sintaxe). Checagem manual: cada aresta usa `-->`, subgraphs têm
`end`, ids de nó são únicos.

- [ ] **Step 3: Linkar no README**

Em `README.md`, localizar a lista de docs (procurar por `docs/DEPLOY.md` ou
`docs/ARCHITECTURE.md`) e adicionar uma linha análoga no mesmo estilo:

Run: `grep -n "docs/" README.md`

Adicionar (ajustando ao formato encontrado, ex. item de lista):

```markdown
- [docs/SCALING.md](docs/SCALING.md) — escala horizontal (réplicas + Postgres) e o `QUARK_NODE_ID`
```

- [ ] **Step 4: Commit**

```bash
git add docs/SCALING.md README.md
git commit -m "docs(scale): SCALING.md com os 3 formatos, Mermaid, capacidade e limites honestos"
```

---

## Self-Review (preenchido pelo autor do plano)

**Cobertura da spec:**
- Postgres multi-réplica (verificar) → Task 3 (unicidade concorrente + create-A/redirect-B). ✓
- LMDB node-id opt-in, default preservado → Task 1 (helpers) + Task 2 (wiring). ✓
- Invariante de não-vazamento → `compose_id` erra antes de compor/gravar (Task 1 test `compose_id_estouro...`; Task 2 usa antes do `put`). ✓
- Validação de startup → `parse_node_id` erra → `open` propaga → `main` fail-fast. ✓
- Exaustão → 507 → Task 2 Step 6. ✓
- docs/SCALING.md + Mermaid + tudo-ou-nada + limites → Task 4. ✓
- Nenhuma mudança em `permute` → confirmado (nenhuma task toca `src/permute.rs`). ✓

**Placeholders:** nenhum — todo passo de código traz o código completo.

**Consistência de tipos:** `open_with_node_id(&Path, Option<u8>)`, `compose_id(Option<u8>, u64) -> Result<u64, StoreError>`, `parse_node_id(Option<String>) -> Result<Option<u8>, StoreError>`, `StoreError::IdSpaceExhausted` — usados de forma idêntica entre Task 1, 2 e 3.
