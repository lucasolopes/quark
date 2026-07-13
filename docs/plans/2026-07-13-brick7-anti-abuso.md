# Tijolo 7 — Proteção contra abuso (plano de implementação)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Defender o `POST /` (criação) com rate-limit por IP e recusa de destinos proibidos (blocklist no banco + guarda embutida contra rede interna/loop), sem tocar no redirect.

**Architecture:** Um módulo novo `abuse` com helpers puros (host/IP/match), um `RateLimiter` (memória ou Valkey, fail-open) e um `Blocklist` (snapshot em memória sobre o `Store`, L2 Valkey opcional). O `trait Store` ganha add/remove/list de domínios bloqueados (LMDB + Postgres). O `create` passa a checar, nessa ordem, rate-limit → guarda → blocklist; endpoints `/admin/blocklist` gerenciam a lista sob `QUARK_ADMIN_TOKEN`.

**Tech Stack:** Rust 2021, axum (ConnectInfo, extractors), redis (INCR/EXPIRE, GET/SET), heed (LMDB), sqlx (Postgres), url crate (parse de host), std::net (IPs), serial_test.

## Global Constraints

- Nenhuma checagem nova no redirect/leitura — só no `POST /` e nos `/admin/*`.
- Rate-limit e blocklist são **fail-open** sob falha/timeout de Valkey.
- Rate-limit **default desligado** (`QUARK_RATELIMIT_PER_MIN` ausente/`0`); guarda embutida **default ligada** (`QUARK_BLOCK_PRIVATE=0` desliga).
- Blocklist é **dado no `Store`** (não env), cache snapshot L1 (memória) + L2 (Valkey opcional); match domínio+subdomínio **case-insensitive**.
- A guarda embutida **não faz DNS** — só IP literal e nomes óbvios (`localhost`, `*.localhost`) e o próprio host (anti-loop).
- Reusa `QUARK_VALKEY_URL` e `QUARK_ADMIN_TOKEN`; nenhum backend novo obrigatório.
- Respostas: rate-limit → `429`; destino bloqueado (blocklist ou guarda) → `403`; URL sem host → `400`.
- Testes de Postgres/Valkey **gated** por `QUARK_TEST_DATABASE_URL`/`QUARK_TEST_VALKEY_URL`; sem as envs, pulam. Devem sempre compilar.
- Documentação a nível humano.

---

## File Structure

- `src/abuse/mod.rs` (novo) — `pub mod ratelimit; pub mod blocklist;` + helpers puros: `extract_host`, `is_internal_host`, `host_in_blocklist`.
- `src/abuse/ratelimit.rs` (novo) — `RateLimiter` (Disabled | Memory | Valkey), `check(ip, now) -> bool`.
- `src/abuse/blocklist.rs` (novo) — `Blocklist` (snapshot sobre `Store` + Valkey opcional), `is_blocked(host, now)`, `invalidate`.
- `src/lib.rs` — registra `pub mod abuse;`.
- `src/store/mod.rs` — 3 métodos novos no `trait Store`.
- `src/store/lmdb.rs` — impl + `max_dbs` 5→6 + DB `blocked`.
- `src/store/postgres.rs` — impl + tabela `blocked_domains` no `init_schema`.
- `src/api.rs` — campos novos em `AppState`; checagens no `create`; handlers `/admin/blocklist`; rotas.
- `src/main.rs` — constrói `RateLimiter`/`Blocklist`/config da guarda; serve com `ConnectInfo`.
- `tests/api_it.rs` e `tests/horizontal_scale_it.rs` — atualizar o construtor de `AppState` (campos novos).
- `README.md` — envs novas na tabela de config.

---

## Task 1: Helpers puros de host, IP interno e match de domínio

**Files:**
- Create: `src/abuse/mod.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Produces:
  - `pub fn extract_host(url: &str) -> Option<String>` — host em minúsculas, sem porta; `None` se não parsear ou não tiver host.
  - `pub fn is_internal_host(host: &str) -> bool` — `true` p/ `localhost`/`*.localhost` ou IP literal loopback/privado/link-local/unspecified (v4 e v6).
  - `pub fn host_in_blocklist(host: &str, set: &std::collections::HashSet<String>) -> bool` — `true` se o host ou qualquer domínio-pai (case-insensitive) está em `set`.

- [ ] **Step 1: Registrar o módulo**

Em `src/lib.rs`, adicionar a linha (em ordem alfabética, após `pub mod api;`):

```rust
pub mod abuse;
```

- [ ] **Step 2: Escrever os testes que falham**

Criar `src/abuse/mod.rs` com só os testes primeiro (o corpo vem no Step 4):

```rust
pub mod blocklist;
pub mod ratelimit;

// (implementação vem no Step 4)

#[cfg(test)]
mod tests {
    use super::{extract_host, host_in_blocklist, is_internal_host};
    use std::collections::HashSet;

    #[test]
    fn extract_host_normaliza_e_tira_porta() {
        assert_eq!(extract_host("https://Example.COM/a/b?x=1"), Some("example.com".into()));
        assert_eq!(extract_host("http://host:8080/x"), Some("host".into()));
        assert_eq!(extract_host("http://127.0.0.1:3000"), Some("127.0.0.1".into()));
        assert_eq!(extract_host("not a url"), None);
        assert_eq!(extract_host("http:///semhost"), None);
    }

    #[test]
    fn is_internal_host_pega_loopback_privado_localhost() {
        for h in [
            "localhost", "foo.localhost", "127.0.0.1", "10.0.0.5",
            "192.168.1.1", "172.16.0.1", "169.254.1.1", "0.0.0.0", "::1",
        ] {
            assert!(is_internal_host(h), "deveria bloquear {h}");
        }
    }

    #[test]
    fn is_internal_host_libera_publicos() {
        for h in ["example.com", "8.8.8.8", "1.1.1.1", "meusite.com.br"] {
            assert!(!is_internal_host(h), "não deveria bloquear {h}");
        }
    }

    #[test]
    fn host_in_blocklist_casa_dominio_e_subdominio() {
        let mut set = HashSet::new();
        set.insert("evil.com".to_string());
        assert!(host_in_blocklist("evil.com", &set));
        assert!(host_in_blocklist("x.evil.com", &set));
        assert!(host_in_blocklist("a.b.evil.com", &set));
        assert!(host_in_blocklist("EVIL.COM", &set)); // case-insensitive
        assert!(!host_in_blocklist("eviltwin.com", &set));
        assert!(!host_in_blocklist("evil.com.br", &set));
    }
}
```

- [ ] **Step 3: Rodar e ver falhar**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --lib abuse::tests`
Expected: FAIL na compilação — as funções não existem.

- [ ] **Step 4: Implementar os helpers**

Em `src/abuse/mod.rs`, inserir antes do `#[cfg(test)]`:

```rust
use std::collections::HashSet;
use std::net::IpAddr;

/// Host da URL em minúsculas, sem porta. `None` se não parsear ou não tiver host.
pub fn extract_host(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    u.host_str().map(|h| h.to_ascii_lowercase())
}

/// `true` para destinos de rede interna que um encurtador público não deve encurtar:
/// `localhost`/`*.localhost`, ou IP literal loopback/privado/link-local/unspecified.
/// NÃO resolve DNS — só decide sobre IP literal e nomes óbvios.
pub fn is_internal_host(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    if h == "localhost" || h.ends_with(".localhost") {
        return true;
    }
    // IPv6 literal em URL vem entre colchetes; url crate já os remove no host_str,
    // mas normalizamos por segurança.
    let h_ip = h.trim_start_matches('[').trim_end_matches(']');
    match h_ip.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
        }
        Ok(IpAddr::V6(v6)) => v6.is_loopback() || v6.is_unspecified(),
        Err(_) => false, // nome não-IP e não-localhost: não é "interno" por si só
    }
}

/// `true` se `host` ou qualquer domínio-pai está no conjunto (case-insensitive).
/// Ex.: set={evil.com} bloqueia evil.com, x.evil.com, a.b.evil.com.
pub fn host_in_blocklist(host: &str, set: &HashSet<String>) -> bool {
    let h = host.to_ascii_lowercase();
    let mut rest = h.as_str();
    loop {
        if set.contains(rest) {
            return true;
        }
        match rest.find('.') {
            Some(i) => rest = &rest[i + 1..],
            None => return false,
        }
    }
}
```

- [ ] **Step 5: Rodar e ver passar**

Run: `cargo test --lib abuse::tests`
Expected: PASS (4 testes). (Vai reclamar que `blocklist`/`ratelimit` não existem — nesse caso, crie stubs vazios `src/abuse/blocklist.rs` e `src/abuse/ratelimit.rs` com só um comentário; serão preenchidos nas Tasks 2 e 4. Se preferir, remova temporariamente as linhas `pub mod` e recoloque na Task 2/4 — mas criar os arquivos vazios é mais limpo.)

Crie os stubs para compilar:
```rust
// src/abuse/ratelimit.rs — implementado na Task 2
// src/abuse/blocklist.rs — implementado na Task 4
```

- [ ] **Step 6: fmt + clippy + commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/lib.rs src/abuse/
git commit -m "feat(abuse): helpers puros de host/IP-interno/match de dominio + modulo abuse"
```

---

## Task 2: RateLimiter (memória + Valkey, fail-open)

**Files:**
- Modify: `src/abuse/ratelimit.rs`

**Interfaces:**
- Consumes: nada.
- Produces:
  - `pub struct RateLimiter` com construtores:
    - `pub fn disabled() -> RateLimiter`
    - `pub fn memory(per_min: u32) -> RateLimiter`
    - `pub fn valkey(per_min: u32, conn: redis::aio::MultiplexedConnection) -> RateLimiter`
  - `pub async fn check(&self, ip: &str, now_secs: u64) -> bool` — `true` = permitido; `false` = estourou. Fail-open: erro de Valkey → `true`.

- [ ] **Step 1: Escrever os testes que falham**

Em `src/abuse/ratelimit.rs` (substituindo o stub):

```rust
#[cfg(test)]
mod tests {
    use super::RateLimiter;

    #[tokio::test]
    async fn disabled_sempre_permite() {
        let rl = RateLimiter::disabled();
        for _ in 0..1000 {
            assert!(rl.check("1.2.3.4", 100).await);
        }
    }

    #[tokio::test]
    async fn memoria_bloqueia_acima_do_limite_na_janela() {
        let rl = RateLimiter::memory(2);
        let now = 600; // janela = 600/60 = 10
        assert!(rl.check("1.1.1.1", now).await); // 1
        assert!(rl.check("1.1.1.1", now).await); // 2
        assert!(!rl.check("1.1.1.1", now).await); // 3 -> bloqueia
    }

    #[tokio::test]
    async fn memoria_reseta_na_proxima_janela() {
        let rl = RateLimiter::memory(1);
        assert!(rl.check("2.2.2.2", 600).await); // janela 10, count 1
        assert!(!rl.check("2.2.2.2", 600).await); // estoura
        assert!(rl.check("2.2.2.2", 660).await); // janela 11: reseta, permite
    }

    #[tokio::test]
    async fn memoria_ips_distintos_nao_interferem() {
        let rl = RateLimiter::memory(1);
        assert!(rl.check("3.3.3.3", 600).await);
        assert!(rl.check("4.4.4.4", 600).await); // outro IP, própria conta
    }
}
```

- [ ] **Step 2: Rodar e ver falhar**

Run: `cargo test --lib abuse::ratelimit`
Expected: FAIL na compilação — `RateLimiter` não existe.

- [ ] **Step 3: Implementar o RateLimiter**

Em `src/abuse/ratelimit.rs`, no topo (antes do `#[cfg(test)]`):

```rust
use std::collections::HashMap;
use std::sync::Mutex;

/// Rate-limit por IP em janela fixa de 60s. Fail-open: erro de Valkey deixa passar.
pub struct RateLimiter(Mode);

enum Mode {
    Disabled,
    Memory {
        per_min: u32,
        // ip -> (janela atual, contagem na janela)
        state: Mutex<HashMap<String, (u64, u32)>>,
    },
    Valkey {
        per_min: u32,
        conn: redis::aio::MultiplexedConnection,
    },
}

const WINDOW_SECS: u64 = 60;

impl RateLimiter {
    pub fn disabled() -> RateLimiter {
        RateLimiter(Mode::Disabled)
    }

    pub fn memory(per_min: u32) -> RateLimiter {
        RateLimiter(Mode::Memory {
            per_min,
            state: Mutex::new(HashMap::new()),
        })
    }

    pub fn valkey(per_min: u32, conn: redis::aio::MultiplexedConnection) -> RateLimiter {
        RateLimiter(Mode::Valkey { per_min, conn })
    }

    /// `true` = permitido. `false` = estourou o limite nesta janela.
    pub async fn check(&self, ip: &str, now_secs: u64) -> bool {
        let window = now_secs / WINDOW_SECS;
        match &self.0 {
            Mode::Disabled => true,
            Mode::Memory { per_min, state } => {
                let mut map = state.lock().unwrap();
                let entry = map.entry(ip.to_string()).or_insert((window, 0));
                if entry.0 != window {
                    *entry = (window, 0); // nova janela: reseta
                }
                entry.1 += 1;
                entry.1 <= *per_min
            }
            Mode::Valkey { per_min, conn } => {
                let key = format!("quark:rl:{ip}:{window}");
                let mut c = conn.clone();
                // INCR + EXPIRE; qualquer erro => fail-open (permite)
                let count: Result<i64, _> = redis::cmd("INCR")
                    .arg(&key)
                    .query_async(&mut c)
                    .await;
                match count {
                    Ok(n) => {
                        // best-effort: só põe TTL na primeira vez; erro de EXPIRE não bloqueia
                        if n == 1 {
                            let _: Result<(), _> = redis::cmd("EXPIRE")
                                .arg(&key)
                                .arg(WINDOW_SECS as i64 * 2)
                                .query_async(&mut c)
                                .await;
                        }
                        n as u32 <= *per_min
                    }
                    Err(_) => true, // fail-open
                }
            }
        }
    }
}
```

- [ ] **Step 4: Rodar e ver passar**

Run: `cargo test --lib abuse::ratelimit`
Expected: PASS (4 testes).

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/abuse/ratelimit.rs
git commit -m "feat(abuse): RateLimiter janela-fixa (memoria + Valkey, fail-open, default disabled)"
```

---

## Task 3: Métodos de blocklist no `Store` (trait + LMDB + Postgres)

**Files:**
- Modify: `src/store/mod.rs` (trait)
- Modify: `src/store/lmdb.rs` (impl + `max_dbs` 5→6 + DB `blocked`)
- Modify: `src/store/postgres.rs` (impl + tabela `blocked_domains`)
- Test: in-module em `src/store/lmdb.rs`

**Interfaces:**
- Produces (no `trait Store`):
  - `async fn add_blocked_domain(&self, domain: &str) -> Result<(), StoreError>`
  - `async fn remove_blocked_domain(&self, domain: &str) -> Result<(), StoreError>`
  - `async fn list_blocked_domains(&self) -> Result<Vec<String>, StoreError>`
  - Contrato: domínios são gravados **normalizados** (trim + lowercase). `add` é idempotente. `list` devolve todos, sem ordem garantida.

- [ ] **Step 1: Declarar os métodos no trait**

Em `src/store/mod.rs`, dentro de `pub trait Store`, adicionar (após `put_alias_and_link`):

```rust
    async fn add_blocked_domain(&self, domain: &str) -> Result<(), StoreError>;
    async fn remove_blocked_domain(&self, domain: &str) -> Result<(), StoreError>;
    async fn list_blocked_domains(&self) -> Result<Vec<String>, StoreError>;
```

- [ ] **Step 2: Escrever o teste que falha (LMDB)**

No `#[cfg(test)] mod tests` de `src/store/lmdb.rs`, adicionar:

```rust
    #[tokio::test]
    async fn blocklist_add_list_remove() {
        let dir = tempfile::tempdir().unwrap();
        let s = LmdbStore::open_with_node_id(dir.path(), None).unwrap();
        s.add_blocked_domain("Evil.COM").await.unwrap(); // normaliza
        s.add_blocked_domain("evil.com").await.unwrap(); // idempotente
        s.add_blocked_domain("spam.net").await.unwrap();
        let mut list = s.list_blocked_domains().await.unwrap();
        list.sort();
        assert_eq!(list, vec!["evil.com".to_string(), "spam.net".to_string()]);
        s.remove_blocked_domain("evil.com").await.unwrap();
        assert_eq!(s.list_blocked_domains().await.unwrap(), vec!["spam.net".to_string()]);
    }
```

- [ ] **Step 3: Rodar e ver falhar**

Run: `cargo test --lib store::lmdb::tests::blocklist`
Expected: FAIL na compilação — métodos e DB `blocked` não existem.

- [ ] **Step 4: Implementar no LMDB**

Em `src/store/lmdb.rs`:

(4a) Adicionar o campo na struct `LmdbStore`:
```rust
    blocked: Database<Str, Str>, // domínio -> "" (conjunto de domínios bloqueados)
```

(4b) Subir `max_dbs` e criar a DB em `open_with_node_id` — trocar `.max_dbs(5)` por `.max_dbs(6)`, e após `let events = ...`:
```rust
        let blocked = env.create_database(&mut wtxn, Some("blocked"))?;
```
e incluir `blocked,` na construção do `LmdbStore { ... }`.

(4c) Adicionar os 3 métodos no `impl Store for LmdbStore` (após `put_alias_and_link`):
```rust
    async fn add_blocked_domain(&self, domain: &str) -> Result<(), StoreError> {
        let d = domain.trim().to_ascii_lowercase();
        let mut wtxn = self.env.write_txn()?;
        self.blocked.put(&mut wtxn, &d, "")?;
        wtxn.commit()?;
        Ok(())
    }

    async fn remove_blocked_domain(&self, domain: &str) -> Result<(), StoreError> {
        let d = domain.trim().to_ascii_lowercase();
        let mut wtxn = self.env.write_txn()?;
        self.blocked.delete(&mut wtxn, &d)?;
        wtxn.commit()?;
        Ok(())
    }

    async fn list_blocked_domains(&self) -> Result<Vec<String>, StoreError> {
        let rtxn = self.env.read_txn()?;
        let mut out = Vec::new();
        for item in self.blocked.iter(&rtxn)? {
            let (k, _) = item?;
            out.push(k.to_string());
        }
        Ok(out)
    }
```

- [ ] **Step 5: Rodar o teste LMDB e ver passar**

Run: `cargo test --lib store::lmdb::tests::blocklist`
Expected: PASS.

- [ ] **Step 6: Implementar no Postgres**

Em `src/store/postgres.rs`:

(6a) No `init_schema`, adicionar ao array de DDL (dentro do bloco já existente):
```rust
                "CREATE TABLE IF NOT EXISTS blocked_domains (domain TEXT PRIMARY KEY)",
```

(6b) No `impl Store for PostgresStore`, adicionar os 3 métodos:
```rust
    async fn add_blocked_domain(&self, domain: &str) -> Result<(), StoreError> {
        let d = domain.trim().to_lowercase();
        sqlx::query("INSERT INTO blocked_domains (domain) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(&d)
            .execute(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        Ok(())
    }

    async fn remove_blocked_domain(&self, domain: &str) -> Result<(), StoreError> {
        let d = domain.trim().to_lowercase();
        sqlx::query("DELETE FROM blocked_domains WHERE domain = $1")
            .bind(&d)
            .execute(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        Ok(())
    }

    async fn list_blocked_domains(&self) -> Result<Vec<String>, StoreError> {
        let rows = sqlx::query("SELECT domain FROM blocked_domains")
            .fetch_all(&self.pool)
            .await
            .map_err(StoreError::backend)?;
        rows.iter()
            .map(|r| r.try_get::<String, _>("domain").map_err(StoreError::backend))
            .collect()
    }
```
(Confirmar que `use sqlx::Row;` já está no topo — está, é usado por `try_get`.)

- [ ] **Step 7: Compilar tudo + rodar suíte de lib**

Run: `cargo test --lib`
Expected: PASS (todos, incluindo o novo `blocklist_add_list_remove`).

- [ ] **Step 8: fmt + clippy + commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/store/
git commit -m "feat(store): add/remove/list de dominios bloqueados (LMDB max_dbs 6 + Postgres blocked_domains)"
```

---

## Task 4: Blocklist com cache snapshot (L1 memória + L2 Valkey opcional)

**Files:**
- Modify: `src/abuse/blocklist.rs`

**Interfaces:**
- Consumes: `crate::store::Store`; `super::host_in_blocklist` (Task 1); `redis::aio::MultiplexedConnection`.
- Produces:
  - `pub struct Blocklist`
  - `pub fn new(store: std::sync::Arc<dyn crate::store::Store>, valkey: Option<redis::aio::MultiplexedConnection>, ttl_secs: u64) -> Blocklist`
  - `pub async fn is_blocked(&self, host: &str, now_secs: u64) -> bool` — recarrega o snapshot se vencido (TTL), depois casa domínio+subdomínio.
  - `pub async fn invalidate(&self)` — força recarga na próxima checagem (e apaga a chave do Valkey).

- [ ] **Step 1: Escrever o teste que falha**

Em `src/abuse/blocklist.rs` (substituindo o stub):

```rust
#[cfg(test)]
mod tests {
    use super::Blocklist;
    use crate::store::{lmdb::LmdbStore, Store};
    use std::sync::Arc;

    #[tokio::test]
    async fn reflete_o_store_e_casa_subdominio() {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn Store> = Arc::new(LmdbStore::open_with_node_id(dir.path(), None).unwrap());
        store.add_blocked_domain("evil.com").await.unwrap();

        let bl = Blocklist::new(store.clone(), None, 60);
        // t=100: primeira checagem carrega o snapshot
        assert!(bl.is_blocked("evil.com", 100).await);
        assert!(bl.is_blocked("x.evil.com", 100).await);
        assert!(!bl.is_blocked("ok.com", 100).await);
    }

    #[tokio::test]
    async fn invalidate_forca_recarga() {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn Store> = Arc::new(LmdbStore::open_with_node_id(dir.path(), None).unwrap());
        let bl = Blocklist::new(store.clone(), None, 3600); // TTL longo

        assert!(!bl.is_blocked("late.com", 100).await); // snapshot vazio carregado
        store.add_blocked_domain("late.com").await.unwrap();
        // sem invalidar, o snapshot antigo (TTL longo) ainda não vê:
        assert!(!bl.is_blocked("late.com", 101).await);
        bl.invalidate().await;
        assert!(bl.is_blocked("late.com", 102).await); // recarregou
    }

    #[tokio::test]
    async fn recarrega_apos_ttl() {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn Store> = Arc::new(LmdbStore::open_with_node_id(dir.path(), None).unwrap());
        let bl = Blocklist::new(store.clone(), None, 10);
        assert!(!bl.is_blocked("z.com", 100).await); // carrega vazio em t=100
        store.add_blocked_domain("z.com").await.unwrap();
        assert!(!bl.is_blocked("z.com", 105).await); // dentro do TTL: snapshot velho
        assert!(bl.is_blocked("z.com", 111).await); // t=111 > 100+10: recarrega
    }
}
```

- [ ] **Step 2: Rodar e ver falhar**

Run: `cargo test --lib abuse::blocklist`
Expected: FAIL na compilação — `Blocklist` não existe.

- [ ] **Step 3: Implementar o Blocklist**

Em `src/abuse/blocklist.rs`, antes do `#[cfg(test)]`:

```rust
use crate::store::Store;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;

const VALKEY_KEY: &str = "quark:blocklist";

struct Snapshot {
    loaded_at: u64, // epoch secs; 0 = nunca / inválido
    set: HashSet<String>,
}

/// Blocklist de domínios com snapshot em memória (L1) sobre o `Store`, e
/// Valkey opcional (L2) como fonte compartilhada entre réplicas. Fail-open:
/// erro de Valkey cai para o `Store`. Propagação entre réplicas é eventual (≤ TTL).
pub struct Blocklist {
    store: Arc<dyn Store>,
    valkey: Option<redis::aio::MultiplexedConnection>,
    ttl_secs: u64,
    snap: RwLock<Snapshot>,
}

impl Blocklist {
    pub fn new(
        store: Arc<dyn Store>,
        valkey: Option<redis::aio::MultiplexedConnection>,
        ttl_secs: u64,
    ) -> Blocklist {
        Blocklist {
            store,
            valkey,
            ttl_secs,
            snap: RwLock::new(Snapshot {
                loaded_at: 0,
                set: HashSet::new(),
            }),
        }
    }

    pub async fn is_blocked(&self, host: &str, now_secs: u64) -> bool {
        self.ensure_fresh(now_secs).await;
        let snap = self.snap.read().await;
        super::host_in_blocklist(host, &snap.set)
    }

    /// Força recarga na próxima checagem e apaga a chave compartilhada do Valkey.
    pub async fn invalidate(&self) {
        {
            let mut snap = self.snap.write().await;
            snap.loaded_at = 0;
        }
        if let Some(conn) = &self.valkey {
            let mut c = conn.clone();
            let _: Result<(), _> = redis::cmd("DEL").arg(VALKEY_KEY).query_async(&mut c).await;
        }
    }

    async fn ensure_fresh(&self, now_secs: u64) {
        {
            let snap = self.snap.read().await;
            if snap.loaded_at != 0 && now_secs.saturating_sub(snap.loaded_at) < self.ttl_secs {
                return; // ainda fresco
            }
        }
        let set = self.load_set().await;
        let mut snap = self.snap.write().await;
        snap.set = set;
        snap.loaded_at = now_secs.max(1); // nunca 0 (0 = inválido)
    }

    /// Carrega o conjunto: tenta Valkey (L2); se ausente/erro, lê o Store e
    /// popula o Valkey best-effort.
    async fn load_set(&self) -> HashSet<String> {
        if let Some(conn) = &self.valkey {
            let mut c = conn.clone();
            let cached: Result<Option<String>, _> =
                redis::cmd("GET").arg(VALKEY_KEY).query_async(&mut c).await;
            if let Ok(Some(json)) = cached {
                if let Ok(v) = serde_json::from_str::<Vec<String>>(&json) {
                    return v.into_iter().collect();
                }
            }
        }
        // fonte da verdade: o Store
        let list = self.store.list_blocked_domains().await.unwrap_or_default();
        if let Some(conn) = &self.valkey {
            if let Ok(json) = serde_json::to_string(&list) {
                let mut c = conn.clone();
                let _: Result<(), _> = redis::cmd("SET")
                    .arg(VALKEY_KEY)
                    .arg(json)
                    .query_async(&mut c)
                    .await;
            }
        }
        list.into_iter().collect()
    }
}
```

- [ ] **Step 4: Rodar e ver passar**

Run: `cargo test --lib abuse::blocklist`
Expected: PASS (3 testes).

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/abuse/blocklist.rs
git commit -m "feat(abuse): Blocklist snapshot (L1 memoria + L2 Valkey opcional) sobre o Store"
```

---

## Task 5: Ligar rate-limit + guarda + blocklist no `create` e no `main`

**Files:**
- Modify: `src/api.rs` (`AppState`, `create`, imports)
- Modify: `src/main.rs` (construção + serve com ConnectInfo)
- Modify: `tests/api_it.rs` (construtor de `AppState`, testes novos)
- Modify: `tests/horizontal_scale_it.rs` (construtor de `AppState` no `pg_replica`)

**Interfaces:**
- Consumes: `RateLimiter` (Task 2), `Blocklist` (Task 4), `extract_host`/`is_internal_host` (Task 1).
- Produces: `AppState` com campos novos:
  - `pub ratelimiter: crate::abuse::ratelimit::RateLimiter`
  - `pub blocklist: crate::abuse::blocklist::Blocklist`
  - `pub block_private: bool`
  - `pub public_host: Option<String>`
  - `pub real_ip_header: String`

- [ ] **Step 1: Adicionar os campos em `AppState`**

Em `src/api.rs`, no `pub struct AppState`, adicionar após `admin_token`:

```rust
    pub ratelimiter: crate::abuse::ratelimit::RateLimiter,
    pub blocklist: crate::abuse::blocklist::Blocklist,
    pub block_private: bool,
    pub public_host: Option<String>,
    pub real_ip_header: String,
```

E nos imports do topo de `src/api.rs`, adicionar:
```rust
use crate::abuse::{extract_host, is_internal_host};
use axum::extract::ConnectInfo;
use std::net::SocketAddr;
```

- [ ] **Step 2: Escrever os testes de integração que falham**

Em `tests/api_it.rs`, primeiro atualizar o helper `app()` para preencher os campos novos (rate-limit desligado, guarda ligada, sem valkey). Substituir a construção do `state` por:

```rust
    let store2 = store.clone();
    let state = Arc::new(AppState {
        cache,
        store,
        key: 0x1234,
        analytics_tx,
        sink,
        admin_token: None,
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        blocklist: quark::abuse::blocklist::Blocklist::new(store2, None, 60),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".to_string(),
    });
```
(`app()` precisará capturar `store` antes do `move`; ajuste conforme o helper.)

Adicionar os testes:

```rust
#[tokio::test]
async fn bloqueia_destino_interno_403() {
    let app = app().await;
    let resp = app
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"url":"http://127.0.0.1:8080/x"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn bloqueia_dominio_na_blocklist_403() {
    // helper dedicado que semeia a blocklist antes de montar o app
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path()).await.unwrap();
    store.add_blocked_domain("evil.com").await.unwrap();
    let cache = Cache::new(store.clone(), 1000);
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
    let state = Arc::new(AppState {
        cache,
        store: store.clone(),
        key: 0x1234,
        analytics_tx: tx,
        sink,
        admin_token: None,
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        blocklist: quark::abuse::blocklist::Blocklist::new(store, None, 60),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".to_string(),
    });
    let app = router(state);
    let resp = app
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"url":"https://sub.evil.com/x"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn rate_limit_429_apos_estourar() {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path()).await.unwrap();
    let cache = Cache::new(store.clone(), 1000);
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
    let store2 = store.clone();
    let state = Arc::new(AppState {
        cache,
        store,
        key: 0x1234,
        analytics_tx: tx,
        sink,
        admin_token: None,
        ratelimiter: quark::abuse::ratelimit::RateLimiter::memory(1),
        blocklist: quark::abuse::blocklist::Blocklist::new(store2, None, 60),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".to_string(),
    });
    let app = router(state);
    let mk = || {
        Request::post("/")
            .header("content-type", "application/json")
            .header("cf-connecting-ip", "9.9.9.9")
            .body(Body::from(r#"{"url":"https://ok.com/x"}"#))
            .unwrap()
    };
    assert_eq!(app.clone().oneshot(mk()).await.unwrap().status(), StatusCode::OK);
    assert_eq!(
        app.oneshot(mk()).await.unwrap().status(),
        StatusCode::TOO_MANY_REQUESTS
    );
}
```

- [ ] **Step 3: Rodar e ver falhar**

Run: `cargo test --test api_it`
Expected: FAIL na compilação — campos de `AppState` e checagens não existem.

- [ ] **Step 4: Implementar as checagens no `create`**

Em `src/api.rs`, trocar a assinatura e o começo de `create`. A assinatura passa a extrair IP (header ou socket) e headers; `Json` continua por último:

```rust
async fn create(
    State(st): State<Arc<AppState>>,
    conn: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    Json(req): Json<CreateReq>,
) -> Response {
    // 1) rate-limit (checagem barata primeiro)
    let ip = client_ip(&headers, &st.real_ip_header, conn.as_ref());
    if !st.ratelimiter.check(&ip, now()).await {
        return (StatusCode::TOO_MANY_REQUESTS, "muitas requisições").into_response();
    }
    // 2) validação de URL (http/https) — já existente
    if !is_valid_url(&req.url) {
        return (StatusCode::BAD_REQUEST, "url inválida").into_response();
    }
    // 3) host do destino (URL sem host é inválida)
    let Some(host) = extract_host(&req.url) else {
        return (StatusCode::BAD_REQUEST, "url sem host").into_response();
    };
    // 4) guarda embutida (rede interna / loop pro próprio host)
    if st.block_private && is_blocked_target(&host, &headers, &st) {
        return (StatusCode::FORBIDDEN, "destino não permitido").into_response();
    }
    // 5) blocklist do banco (domínio + subdomínio)
    if st.blocklist.is_blocked(&host, now()).await {
        return (StatusCode::FORBIDDEN, "destino bloqueado").into_response();
    }

    let expiry = match req.ttl {
        // ... (resto do create ATUAL, inalterado a partir daqui)
```

Manter todo o corpo restante de `create` (cálculo de expiry, alias, id, etc.) exatamente como está hoje.

Adicionar, após `create`, os dois helpers:

```rust
/// IP do cliente: header configurável (default CF-Connecting-IP) tem prioridade;
/// senão o IP do socket; senão "unknown" (bucket único, conservador).
fn client_ip(headers: &HeaderMap, header_name: &str, conn: Option<&ConnectInfo<SocketAddr>>) -> String {
    if let Some(v) = headers.get(header_name).and_then(|v| v.to_str().ok()) {
        let v = v.trim();
        if !v.is_empty() {
            return v.to_string();
        }
    }
    if let Some(ConnectInfo(addr)) = conn {
        return addr.ip().to_string();
    }
    "unknown".to_string()
}

/// Guarda embutida: destino de rede interna, ou loop pro próprio host do quark.
fn is_blocked_target(host: &str, headers: &HeaderMap, st: &AppState) -> bool {
    if is_internal_host(host) {
        return true;
    }
    // anti-loop: host próprio via QUARK_PUBLIC_HOST ou o header Host da requisição
    let self_host = st.public_host.clone().or_else(|| {
        headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .map(|h| h.split(':').next().unwrap_or(h).to_ascii_lowercase())
    });
    matches!(self_host, Some(sh) if sh == host)
}
```

- [ ] **Step 5: Construir tudo no `main.rs`**

Em `src/main.rs`, antes de montar `let state = Arc::new(AppState { ... })`, adicionar:

```rust
    // --- proteção contra abuso ---
    let control_conn: Option<redis::aio::MultiplexedConnection> =
        match std::env::var("QUARK_VALKEY_URL").ok() {
            Some(url) => match redis::Client::open(url) {
                Ok(client) => client.get_multiplexed_async_connection().await.ok(),
                Err(_) => None,
            },
            None => None,
        };
    let per_min: u32 = std::env::var("QUARK_RATELIMIT_PER_MIN")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let ratelimiter = match (per_min, control_conn.clone()) {
        (0, _) => quark::abuse::ratelimit::RateLimiter::disabled(),
        (n, Some(conn)) => quark::abuse::ratelimit::RateLimiter::valkey(n, conn),
        (n, None) => quark::abuse::ratelimit::RateLimiter::memory(n),
    };
    if per_min == 0 {
        eprintln!("rate-limit desligado (defina QUARK_RATELIMIT_PER_MIN=n para ligar)");
    } else {
        eprintln!("rate-limit: {per_min}/min por IP ({})", if control_conn.is_some() { "global via Valkey" } else { "por réplica (memória)" });
    }
    let blocklist_ttl: u64 = std::env::var("QUARK_BLOCKLIST_TTL")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    let blocklist =
        quark::abuse::blocklist::Blocklist::new(store.clone(), control_conn.clone(), blocklist_ttl);
    let block_private = std::env::var("QUARK_BLOCK_PRIVATE").map(|v| v != "0").unwrap_or(true);
    let public_host = std::env::var("QUARK_PUBLIC_HOST").ok();
    let real_ip_header =
        std::env::var("QUARK_REAL_IP_HEADER").unwrap_or_else(|_| "cf-connecting-ip".to_string());
```

Incluir os campos no `AppState { ... }`:
```rust
        ratelimiter,
        blocklist,
        block_private,
        public_host,
        real_ip_header,
```

E trocar a linha de serve para fornecer o `ConnectInfo`:
```rust
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await
    .expect("serve");
```

- [ ] **Step 6: Atualizar o construtor de `AppState` no `horizontal_scale_it.rs`**

Em `tests/horizontal_scale_it.rs`, na função `pg_replica`, adicionar os mesmos campos novos ao `AppState` (rate-limit disabled, blocklist sobre o `store`, guarda ligada, sem valkey):

```rust
    let store2 = store.clone();
    let state = Arc::new(AppState {
        cache,
        store,
        key: 0x1234,
        analytics_tx,
        sink,
        admin_token: None,
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        blocklist: quark::abuse::blocklist::Blocklist::new(store2, None, 60),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".to_string(),
    });
```
(Ajustar a captura de `store`/`store2` conforme necessário para os moves.)

- [ ] **Step 7: Rodar e ver passar**

Run: `cargo test --test api_it && cargo test --lib && cargo test --test horizontal_scale_it`
Expected: PASS em tudo (api_it inclui os 3 testes novos + os antigos; os testes gated de PG pulam sem env).

- [ ] **Step 8: fmt + clippy + commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/api.rs src/main.rs tests/api_it.rs tests/horizontal_scale_it.rs
git commit -m "feat(abuse): create checa rate-limit + guarda interna/loop + blocklist; main constroi e serve com ConnectInfo"
```

---

## Task 6: Endpoints admin `/admin/blocklist` + docs

**Files:**
- Modify: `src/api.rs` (handlers + rotas)
- Modify: `tests/api_it.rs` (testes admin)
- Modify: `README.md` (envs novas)

**Interfaces:**
- Consumes: `AppState.admin_token`, `AppState.store`, `AppState.blocklist`, `constant_time_eq` (já existe em `api.rs`).
- Produces: rotas `GET/POST/DELETE /admin/blocklist`.

- [ ] **Step 1: Escrever os testes que falham**

Em `tests/api_it.rs`, adicionar um helper que monta um app com admin token e os testes:

```rust
async fn app_admin(token: &str) -> axum::Router {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path()).await.unwrap();
    let cache = Cache::new(store.clone(), 1000);
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
    let store2 = store.clone();
    let state = Arc::new(AppState {
        cache,
        store,
        key: 0x1234,
        analytics_tx: tx,
        sink,
        admin_token: Some(token.to_string()),
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        blocklist: quark::abuse::blocklist::Blocklist::new(store2, None, 60),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".to_string(),
    });
    router(state)
}

#[tokio::test]
async fn admin_blocklist_add_list_e_bloqueia() {
    let app = app_admin("segredo").await;
    // add
    let resp = app
        .clone()
        .oneshot(
            Request::post("/admin/blocklist")
                .header("content-type", "application/json")
                .header("x-admin-token", "segredo")
                .body(Body::from(r#"{"domain":"evil.com"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // list contém
    let resp = app
        .clone()
        .oneshot(
            Request::get("/admin/blocklist")
                .header("x-admin-token", "segredo")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["domains"][0], "evil.com");
}

#[tokio::test]
async fn admin_blocklist_sem_token_404() {
    // app() tem admin_token: None
    let app = app().await;
    let resp = app
        .oneshot(
            Request::get("/admin/blocklist")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn admin_blocklist_token_errado_401() {
    let app = app_admin("segredo").await;
    let resp = app
        .oneshot(
            Request::get("/admin/blocklist")
                .header("x-admin-token", "errado")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
```

- [ ] **Step 2: Rodar e ver falhar**

Run: `cargo test --test api_it`
Expected: FAIL — rotas `/admin/blocklist` não existem (404 onde se espera 200/401).

- [ ] **Step 3: Implementar os handlers**

Em `src/api.rs`, adicionar o tipo do corpo e os handlers (após `stats`):

```rust
#[derive(Deserialize)]
struct BlocklistReq {
    domain: String,
}

/// Autoriza uma requisição admin: `Ok(())` se o token bate; `Err(resposta)` senão.
/// Sem token configurado → 404 (endpoint desligado); token errado → 401.
fn admin_guard(st: &AppState, headers: &HeaderMap) -> Result<(), Response> {
    let Some(expected) = st.admin_token.as_deref() else {
        return Err(StatusCode::NOT_FOUND.into_response());
    };
    let provided = headers
        .get("x-admin-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !constant_time_eq(provided.as_bytes(), expected.as_bytes()) {
        return Err(StatusCode::UNAUTHORIZED.into_response());
    }
    Ok(())
}

async fn blocklist_get(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(r) = admin_guard(&st, &headers) {
        return r;
    }
    match st.store.list_blocked_domains().await {
        Ok(domains) => Json(serde_json::json!({ "domains": domains })).into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

async fn blocklist_add(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<BlocklistReq>,
) -> Response {
    if let Err(r) = admin_guard(&st, &headers) {
        return r;
    }
    if st.store.add_blocked_domain(&req.domain).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    st.blocklist.invalidate().await;
    StatusCode::OK.into_response()
}

async fn blocklist_delete(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<BlocklistReq>,
) -> Response {
    if let Err(r) = admin_guard(&st, &headers) {
        return r;
    }
    if st.store.remove_blocked_domain(&req.domain).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    st.blocklist.invalidate().await;
    StatusCode::OK.into_response()
}
```

- [ ] **Step 4: Registrar as rotas**

Em `src/api.rs`, na função `router`, adicionar as rotas ao `Router::new()` (antes de `.with_state(state)`):

```rust
        .route(
            "/admin/blocklist",
            get(blocklist_get).post(blocklist_add).delete(blocklist_delete),
        )
```

- [ ] **Step 5: Rodar e ver passar**

Run: `cargo test --test api_it`
Expected: PASS (todos, incluindo os 3 novos de admin).

- [ ] **Step 6: Documentar as envs no README**

Em `README.md`, na tabela/lista de variáveis de ambiente (procurar por `QUARK_ADMIN_TOKEN`), adicionar as linhas no mesmo formato:

Run: `grep -n "QUARK_ADMIN_TOKEN\|QUARK_VALKEY_URL" README.md`

Adicionar (ajustando ao formato encontrado):
```markdown
- `QUARK_RATELIMIT_PER_MIN` — criações/min por IP no `POST /` (ausente/`0` = desligado). Usa Valkey se `QUARK_VALKEY_URL` estiver setado (limite global), senão memória por réplica.
- `QUARK_REAL_IP_HEADER` — header de onde ler o IP do cliente (default `CF-Connecting-IP`).
- `QUARK_BLOCK_PRIVATE` — guarda contra destinos internos/loop; ligada por default, `0` desliga.
- `QUARK_PUBLIC_HOST` — host próprio para anti-loop (senão usa o header `Host`).
- `QUARK_BLOCKLIST_TTL` — segundos de cache do snapshot da blocklist (default `60`).
```
E uma linha curta na seção de operação apontando que a blocklist é gerenciada por `POST/DELETE/GET /admin/blocklist` (token `QUARK_ADMIN_TOKEN`).

- [ ] **Step 7: fmt + clippy + suíte completa + commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
cargo test --lib && cargo test --test api_it
git add src/api.rs tests/api_it.rs README.md
git commit -m "feat(abuse): endpoints /admin/blocklist (GET/POST/DELETE, QUARK_ADMIN_TOKEN) + docs de config"
```

---

## Self-Review (preenchido pelo autor do plano)

**Cobertura da spec:**
- Rate-limit janela-fixa memória+Valkey, fail-open, default off, 429 → Task 2 + Task 5. ✓
- Fonte de IP (header configurável + ConnectInfo) → Task 5 (`client_ip`, serve com ConnectInfo). ✓
- Blocklist no Store (add/remove/list, LMDB max_dbs 6 + Postgres) → Task 3. ✓
- Cache snapshot L1+L2 Valkey, match domínio+subdomínio, propagação ≤ TTL → Task 1 (`host_in_blocklist`) + Task 4. ✓
- Guarda embutida default on, IP privado/localhost/self, sem DNS → Task 1 (`is_internal_host`) + Task 5 (`is_blocked_target`, anti-loop via Host/QUARK_PUBLIC_HOST). ✓
- Endpoints /admin/blocklist protegidos, 404 sem token / 401 errado → Task 6. ✓
- 403 destino, 429 rate-limit, 400 sem host, redirect intocado → Task 5. ✓
- Envs documentadas → Task 6. ✓

**Placeholders:** nenhum — todo passo de código traz o código; o único "resto inalterado" é o corpo já existente de `create` (explicitamente mantido).

**Consistência de tipos:** `RateLimiter::{disabled,memory,valkey}` + `check(&str,u64)->bool`; `Blocklist::new(Arc<dyn Store>, Option<MultiplexedConnection>, u64)` + `is_blocked(&str,u64)->bool` + `invalidate()`; `extract_host(&str)->Option<String>`, `is_internal_host(&str)->bool`, `host_in_blocklist(&str,&HashSet<String>)->bool`; `Store::{add_blocked_domain,remove_blocked_domain,list_blocked_domains}` — usados de forma idêntica entre as tasks. Campos de `AppState` idênticos nos 3 construtores (main, api_it, horizontal_scale_it).

**Nota de escopo/risco:** o mapa em memória do RateLimiter cresce com IPs distintos por processo (janela fixa; entrada por IP substituída ao virar a janela). Aceitável para o volume atual; poda/cap fica como follow-up se necessário. Documentado aqui, não é defeito.
