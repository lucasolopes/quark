# Tijolo 3 — Cache L2 (Valkey) — design

**Data:** 2026-07-13
**Status:** spec (usuário delegou tijolos 3-5; arquitetura assentada na pesquisa)
**Programa:** terceiro de 5 tijolos (1. storage ✅ · 2. analytics ✅ · 3. L2 Valkey ← *este* · 4. Postgres · 5. ClickHouse).

## 1. Objetivo

Cache de duas camadas pro caminho de leitura do redirect: **L1 in-process (moka,
já existe) + L2 compartilhado (Valkey)**. Habilita **múltiplas instâncias**
compartilharem o cache quente sem cada uma bater no store. Opt-in: sem
configuração, o comportamento é o de hoje (L1 + store).

**Invariante sagrado:** o redirect continua rápido e **resiliente** — se o Valkey
cair, o L2 é pulado (circuit breaker) e a leitura cai no store; **nunca** falha
nem trava por causa do L2.

## 2. Escopo

**No tijolo:**
- Trait `CacheTier` (get/set) pra abstrair a camada L2.
- `L1L2Cache`: L1 moka + L2 opcional atrás do trait, com circuit breaker.
- Impl `ValkeyTier` (crate `redis`, wire-compatível com Valkey), opt-in via
  `QUARK_VALKEY_URL`.
- Circuit breaker: após N falhas consecutivas no L2, abre por um cooldown (pula
  o L2), depois half-open (tenta de novo).
- TTLs: L1 curto < L2 longo.

**Fora:**
- L2 pro analytics/stats (só o redirect read path).
- Invalidação distribuída sofisticada (os links são imutáveis; expiração via TTL
  do valor cacheado, respeitando o `expiry` do link).

## 3. Por que Valkey (não Redis)

Valkey é fork BSD-3 (Linux Foundation) do Redis, wire-compatível — a crate
`redis` fala com os dois. Pra um projeto OSS, licença importa: Redis foi pra
SSPL/RSAL (não-OSI). Usamos a crate `redis` apontando pro Valkey.

## 4. Arquitetura da leitura (redirect)

```
get(id):
  1. L1 (moka, sync) hit? -> retorna            [caminho quentíssimo, sem rede]
  2. L2 (Valkey) habilitado E breaker fechado?
       hit? -> popula L1, retorna
       erro? -> registra falha no breaker, segue pro store
  3. store.get_link(id) -> se Some: popula L2 (best-effort) e L1; retorna
```
- **L1 hit** (o caso dominante) não toca rede nenhuma — idêntico a hoje.
- **L2** só é consultado no miss de L1, e só se habilitado + breaker fechado.
- Erro de L2 nunca propaga: registra no breaker e cai pro store.

## 5. `CacheTier` trait + `ValkeyTier`

```rust
#[async_trait::async_trait]
pub trait CacheTier: Send + Sync + 'static {
    async fn get(&self, id: u64) -> Result<Option<Record>, TierError>;
    async fn set(&self, id: u64, rec: &Record, ttl_secs: u64) -> Result<(), TierError>;
}
```
- `ValkeyTier`: pool de conexões `redis` (multiplexed async connection). Chave
  `q:<id>`, valor = `Record` json, com `EX ttl_secs`. `TierError` wrap do
  `redis::RedisError`.
- Sem `QUARK_VALKEY_URL`, o L2 não é construído (fica `None`); `L1L2Cache`
  opera só L1+store — comportamento atual, zero dep de rede.

## 6. Circuit breaker (resiliência)

Estado compartilhado (atomics): contador de falhas consecutivas + timestamp de
abertura.
- **Fechado:** consulta L2 normalmente; sucesso zera o contador; falha incrementa.
- Após `BREAKER_THRESHOLD` (default 5) falhas → **Aberto** por `BREAKER_COOLDOWN`
  (default 30s): pula o L2 direto pro store.
- Depois do cooldown → **Half-open:** deixa 1 tentativa; sucesso fecha, falha
  reabre.
Objetivo: um Valkey caído não adiciona latência/timeout a cada request — depois
de poucas falhas, o L2 é ignorado até esfriar.

## 7. TTLs

- **L1 (moka):** TTL curto (default 60s) — refresca do L2/store, mantém coerência
  entre instâncias.
- **L2 (Valkey):** TTL mais longo (default 3600s), **limitado pelo `expiry` do
  link** (link que expira em 30s → TTL do L2 ≤ 30s; nunca serve link vencido).
Links são imutáveis (não há update), então cachear é seguro; só a expiração
importa, e ela é respeitada.

## 8. Config nova

- `QUARK_VALKEY_URL` (ex.: `redis://valkey:6379`). Ausente → L2 desligado.
- Constantes/env: `QUARK_L1_TTL_SECS` (60), `QUARK_L2_TTL_SECS` (3600),
  `BREAKER_THRESHOLD` (5), `BREAKER_COOLDOWN_SECS` (30) — constantes v1, env opcional.

## 9. Arquivos

- Novo: `src/cache/mod.rs` (o `Cache` atual vira `L1L2Cache`; trait `CacheTier`;
  o circuit breaker) — `src/cache.rs` vira diretório.
- Novo: `src/cache/valkey.rs` (`ValkeyTier`, crate `redis`).
- `src/api.rs` / `src/main.rs`: montar o L2 se `QUARK_VALKEY_URL`; `AppState`
  segue com `cache` (agora `L1L2Cache`).
- `Cargo.toml`: `redis = { version = "0.27", features = ["tokio-comp"] }`.

## 10. Testes

- **Unit (sem serviço):** circuit breaker (fecha→abre após N falhas→half-open),
  usando um `CacheTier` fake que falha sob demanda.
- **Unit:** `L1L2Cache` sem L2 (None) = comportamento atual (L1+store).
- **Unit:** cálculo do TTL do L2 limitado pelo `expiry`.
- **Integração (gated, roda com Valkey de pé via `QUARK_TEST_VALKEY_URL`):** set→get
  round-trip no `ValkeyTier`; miss retorna None. `#[ignore]` por default ou skip se
  a env não estiver setada (não quebra o CI sem serviço).
- **Invariante:** redirect responde 302 com o L2 sempre-falhando (fake tier que
  erra) — cai no store, breaker abre, sem latência acumulada.

## 11. CI

Adicionar um serviço Valkey ao job do GitHub Actions (`services: valkey`) e rodar
os testes de integração com `QUARK_TEST_VALKEY_URL` apontando pra ele. Os testes
unit continuam rodando sem serviço.

## 12. Riscos / notas

- **Latência do L2:** um Valkey na mesma rede é sub-ms; o breaker protege contra
  Valkey lento/caído.
- **Coerência L1↔L2:** L1 TTL curto garante refresh; links imutáveis tornam
  staleness só uma questão de expiração (respeitada pelo TTL).
- **`redis` vs `fred`:** escolhida a `redis` (madura, simples, wire-compat Valkey).
- **Pool:** usar `MultiplexedConnection` (uma conexão multiplexada) — simples e
  suficiente pro read path; pool dedicado é otimização futura.
