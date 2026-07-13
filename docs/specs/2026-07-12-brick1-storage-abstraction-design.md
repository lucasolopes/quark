# Tijolo 1 — Abstração de storage — design

**Data:** 2026-07-12
**Status:** spec aprovado (aguardando revisão final do usuário)
**Programa:** primeiro de 5 tijolos da arquitetura plugável do quark
(1. abstração de storage ← *este* · 2. pipeline de analytics · 3. cache L2 Valkey ·
4. backend Postgres · 5. sink ClickHouse).

## 1. Objetivo

Transformar o `Store` — hoje uma struct concreta amarrada ao LMDB — em uma
**trait (interface)**, com o LMDB virando **uma** implementação por trás dela.
É a fundação que deixa Valkey/Postgres/ClickHouse plugarem nos tijolos seguintes.

**Invariante:** refactor de fundação, **sem feature nova** e **sem mudança de
comportamento observável**. O quark segue sendo um único binário, roda com o
backend embutido por default, e os 29 testes existentes continuam verdes.

## 2. Escopo

**No tijolo:**
- Trait `Store` (assíncrona, `Send + Sync`, dyn-compatível).
- LMDB atual movido para `src/store/lmdb.rs`, implementando a trait.
- Factory `open_store(config)` que seleciona o backend em runtime (hoje só `lmdb`).
- `Cache`, `AppState` e `main` passam a depender de `Arc<dyn Store>`.
- Testes adaptados (async) + um teste que exercita via `Arc<dyn Store>`.

**Fora do tijolo (deferido, YAGNI):**
- Abstração de **tiers de cache (L1/L2)** → Tijolo 3 (desenhada com o Valkey em mão).
- Qualquer backend novo (Postgres/Valkey/ClickHouse) → tijolos 3-5.
- Schema relacional, migrações, pool de conexão → Tijolo 4.

## 3. Decisão técnica: trait async + dyn dispatch

A trait é **assíncrona** e o backend é escolhido em **runtime** via
`Arc<dyn Store>`, selecionado por config (`QUARK_STORE`, default `lmdb`).

**Por quê:** os backends futuros (Postgres/Valkey) são async por natureza; uma
interface síncrona os obrigaria a gambiarras de bloqueio. Runtime dispatch deixa
o operador **trocar o backend por config, sem recompilar**. O custo de dispatch
é desprezível perto do I/O, e o caminho quente (redirect) é servido do cache L1
em memória — o async não pesa no redirect.

**Custo:** dependência nova `async-trait` (padrão para traits async dyn-compatíveis).

**Detalhe de implementação do backend LMDB:** heed é síncrono e faz leituras
mmap em microssegundos; as chamadas rodam inline dentro dos `async fn` (sem
`await` real de I/O). Escritas (`next_id`, `put_*`) também são rápidas e rodam
inline neste tijolo. Se a medição futura mostrar bloqueio relevante do executor,
envolver escritas em `spawn_blocking` fica como otimização posterior — não neste
tijolo.

## 4. A interface `Store`

```rust
#[async_trait::async_trait]
pub trait Store: Send + Sync + 'static {
    async fn next_id(&self) -> Result<u64, StoreError>;
    async fn get_link(&self, id: u64) -> Result<Option<Record>, StoreError>;
    async fn put_link(&self, id: u64, rec: &Record) -> Result<(), StoreError>;
    async fn get_alias(&self, alias: &str) -> Result<Option<u64>, StoreError>;
    async fn put_alias_and_link(
        &self,
        alias: &str,
        id: u64,
        rec: &Record,
    ) -> Result<bool, StoreError>;
}
```

- `Record { url: String, expiry: Option<u64>, created: u64 }` — inalterado.
- `StoreError` — inalterado (`Db(heed::Error)` + `Serde(serde_json::Error)`), com
  `Display`/`Error`/`From`. Nota: o nome `Db(heed::Error)` fica específico do
  LMDB por ora; generalizar a variante de erro é problema do Tijolo 4 (quando um
  segundo backend introduzir erros que não são de heed) — não deste tijolo.
- Os métodos avulsos `put_link`/`put_alias` que hoje existem no struct mas não
  são usados fora do próprio módulo/testes: manter apenas os que a API e os
  testes consomem. `put_link` fica na trait (a via não-alias do create usa).
  `put_alias` isolado sai da interface pública se nenhum consumidor externo usar
  (o create usa `put_alias_and_link`); confirmar no código antes de remover.

## 5. Factory / seam de configuração

```rust
// src/store/mod.rs
pub enum StoreBackend { Lmdb }  // ganha Postgres no Tijolo 4

pub async fn open_store(data_path: &Path) -> Result<Arc<dyn Store>, StoreError> {
    // hoje: sempre LMDB. Tijolo 4: match em QUARK_STORE.
    Ok(Arc::new(lmdb::LmdbStore::open(data_path)?))
}
```

O `main` chama `open_store(...)` e guarda `Arc<dyn Store>`. A seleção por
`QUARK_STORE` entra no Tijolo 4; neste tijolo o seam existe mas só resolve `lmdb`.

## 6. Layout de arquivos

```
src/store/
  mod.rs      # trait Store, Record, StoreError, open_store()
  lmdb.rs     # LmdbStore: impl Store (código atual movido pra cá)
src/cache.rs  # Cache passa a embrulhar Arc<dyn Store>
src/api.rs    # AppState.store: Arc<dyn Store>; handlers usam .await
src/main.rs   # monta via open_store(); Arc<dyn Store>
```

O `src/store.rs` atual vira o diretório `src/store/` (mod.rs + lmdb.rs).

## 7. Mudanças nos consumidores

- **`cache.rs`**: `Cache { store: Arc<dyn Store>, hot: Moka<...> }`; `get(id)` vira
  `async` (aguarda `store.get_link(id).await` no miss). O L1 (moka) continua
  síncrono; só o miss aguarda o store.
- **`api.rs`**: `AppState { cache: Cache, store: Arc<dyn Store>, key, ... }`. Os
  handlers já são async; passam a `.await` nas chamadas de store/cache. Nenhuma
  mudança de status/headers/rotas.
- **`main.rs`**: `let store = open_store(path).await?;` (ou sync open dentro de
  factory async), `Arc<dyn Store>` compartilhado.

## 8. Tratamento de erros

Sem mudança semântica. Métodos retornam `Result<_, StoreError>`; a API mapeia
erro de store para `503` como hoje. Sem `panic!`/`unwrap`/`expect` no caminho de
request.

## 9. Testes

- **`tests/store_it.rs`**: passa a `#[tokio::test]` + `.await`. Mesmas asserções
  (put/get link, next_id persiste ao reabrir, alias não sobrescreve, transacional
  sem órfão). Tipar o store nos testes como `Arc<dyn Store>` para exercer o
  dispatch dinâmico de verdade (não a struct concreta).
- **Novo teste**: `store_via_trait_object` — abre o LMDB, coloca atrás de
  `Arc<dyn Store>`, e faz um round-trip (create→get) através do trait object,
  provando que a troca por interface funciona.
- **`tests/api_it.rs`** e testes unitários de `api`/`cache`: ajustados para o
  store/cache async; asserções de comportamento inalteradas.
- Invariante: suíte completa verde; comportamento observável idêntico.

## 10. Dependência nova

- `async-trait = "0.1"` em `[dependencies]`.

## 11. Compatibilidade de dados

Nenhuma migração. O formato do LMDB (`links`/`aliases`/`meta`) é o mesmo; o
`LmdbStore` lê/escreve exatamente como hoje. Um `/data` existente continua válido.

## 12. Riscos / notas

- **Dyn + async**: garantir dyn-compatibilidade via `async_trait` (o macro
  resolve o retorno `Pin<Box<dyn Future>>`). Sem isso, `Arc<dyn Store>` não
  compila.
- **Churn de testes**: a conversão async toca vários arquivos de teste; é
  mecânica, mas ampla — revisar que nenhuma asserção foi enfraquecida no caminho.
- **`StoreError::Db(heed::Error)`** acopla a variante ao LMDB; aceitável neste
  tijolo, generalizado no Tijolo 4.
