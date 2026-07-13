# Tijolo 2 — Pipeline de analytics — design

**Data:** 2026-07-12
**Status:** spec aprovado (aguardando revisão final do usuário)
**Programa:** segundo de 5 tijolos da arquitetura plugável do quark
(1. abstração de storage ✅ · 2. pipeline de analytics ← *este* · 3. cache L2 Valkey ·
4. backend Postgres · 5. sink ClickHouse).

## 1. Objetivo

Analytics rico — contagem, série temporal (por dia), país, device e **eventos crus
por clique** — com **impacto ZERO na latência do redirect** (requisito
inegociável do usuário). A captura é fire-and-forget; toda a agregação e a
persistência acontecem fora do caminho quente. O sink é plugável (embutido
LMDB agora; ClickHouse no Tijolo 5).

**Invariante sagrado:** o `GET /:code` responde 302 idêntico ao de hoje, sem
`await` de I/O, sem lock, sem cálculo — no máximo um `try_send` O(1). Se a
analytics estiver afogada/parada, o redirect **não sente**.

## 2. Escopo

**No tijolo:**
- Captura de clique no caminho de redirect (só no 302), fire-and-forget.
- Worker de fundo que agrega + grava em lote.
- Trait `AnalyticsSink` + implementação embutida `LmdbAnalyticsSink` (compartilha
  o env LMDB do store).
- Agregados por link: total, primeiro/último acesso, por-dia, por-país, por-device
  — guardados pra sempre.
- Eventos crus por link: **últimos N** (ring, default 1000).
- Endpoint `GET /:code/stats` protegido por `QUARK_ADMIN_TOKEN`.

**Fora do tijolo (deferido):**
- Sink ClickHouse → Tijolo 5 (entra atrás do mesmo `AnalyticsSink`).
- Painel/UI, contas → fase de produto.
- Rate-limit/anti-abuso → diferido.

## 3. Impacto zero no caminho quente (o coração)

No handler de `redirect`, **somente quando a resposta é 302** (clique real),
antes de retornar:
- monta `ClickEvent { id: u64, ts: u64, referer: Option<String>, country:
  Option<String>, user_agent: Option<String> }` — todos os campos já disponíveis
  no request; `country` vem do header **`CF-IPCountry`** (a Cloudflare preenche
  de graça; sem base GeoIP);
- faz **`sender.try_send(event)`** num `tokio::sync::mpsc` **limitado**;
- se a fila estiver cheia → **descarta o evento** (best-effort) e segue;
- retorna o 302 exatamente como hoje.

Custo no hit: um `try_send` não-bloqueante. Nenhum `await`, lock, alocação de
peso, nem cálculo (o parse de UA→device é feito no worker, não aqui). 404/410
**não** contam clique.

## 4. Worker de fundo

Uma tarefa `tokio::spawn` separada, criada no startup:
- consome o `Receiver` do canal;
- acumula em memória um mapa de agregados sujos + os eventos crus por id;
- **flush em lote** a cada ~5s **ou** ~500 eventos (o que vier primeiro),
  chamando `sink.record_batch(&events)`;
- faz o parse leve de User-Agent → device (heurística Mobile/Desktop/Other, sem
  dep pesada) **fora** do caminho de request;
- erros de escrita são logados (o log JSON já existe) e **não** afetam o redirect.

## 5. `AnalyticsSink` trait (seam plugável, espelha o `Store` do Tijolo 1)

```rust
#[async_trait::async_trait]
pub trait AnalyticsSink: Send + Sync + 'static {
    async fn record_batch(&self, events: &[ClickEvent]) -> Result<(), StoreError>;
    async fn stats(&self, id: u64) -> Result<Option<Stats>, StoreError>;
}
```
- **`LmdbAnalyticsSink`** (embutida): compartilha o **env LMDB** do store — sobe
  `max_dbs` de 3 pra 5 e adiciona os DBs `stats` e `events`. `record_batch` faz
  o merge dos agregados e o append (com truncamento em N) dos eventos crus numa
  transação de escrita. `stats(id)` lê o agregado + os últimos N eventos.
- **ClickHouse** = Tijolo 5, atrás do mesmo trait, sem tocar captura/worker/endpoint.

## 6. Modelo de dados (2 DBs novos no mesmo env LMDB; `max_dbs=5`)

- `stats`: `u64 id` → `Aggregates { total: u64, first_ts: u64, last_ts: u64,
  per_day: BTreeMap<String,u64>, per_country: BTreeMap<String,u64>,
  per_device: BTreeMap<String,u64> }` (serde_json). Compacto, pra sempre.
- `events`: `u64 id` → `Vec<ClickEvent>` **circular, últimos N** (default 1000),
  truncado no flush.

Os DBs existentes (`links`/`aliases`/`meta`) e seus dados **não** são tocados.

## 7. Leitura — `GET /:code/stats` (protegido)

- Resolve `code → id` com a mesma lógica do redirect (decode numérico primeiro,
  senão alias; código inválido → 404).
- Exige header de admin com `QUARK_ADMIN_TOKEN` (comparação em tempo constante);
  token errado/ausente → **401**.
- Se `QUARK_ADMIN_TOKEN` **não** estiver setado no processo → endpoint
  **desligado (404)**, pra analytics nunca vazar sem opt-in.
- Resposta 200: JSON com os agregados + os últimos N eventos crus.
- Coleta de analytics roda **sempre** (é barata/best-effort); só a **leitura** é
  gated pelo token.

## 8. Config nova

- `QUARK_ADMIN_TOKEN` (opcional; sem ele `/stats` fica off).
- Constantes v1: `ANALYTICS_EVENTS_MAX = 1000`, flush 5s / 500 eventos, canal
  limitado (capacidade ~10_000).

## 9. Tratamento de erros

- Analytics é **best-effort**: fila cheia → descarta; erro de sink → loga e
  segue. **Nunca** propaga pro caminho de request.
- `/stats`: 401 (token), 404 (código inválido ou endpoint desligado), 200 (ok),
  503 (erro de leitura do sink). Sem `panic!`/`unwrap`/`expect` no caminho de
  request.

## 10. Arquivos

- Novo: `src/analytics.rs` — `ClickEvent`, `Aggregates`/`Stats`, trait
  `AnalyticsSink`, `LmdbAnalyticsSink`, o worker (`spawn_worker(rx, sink)`), o
  parse UA→device.
- `src/store/lmdb.rs` — env com `max_dbs=5` + DBs `stats`/`events`; expor o env
  compartilhado pro sink (ou um factory que devolve store + sink do mesmo env).
- `src/api.rs` — redirect emite `try_send`; rota+handler `GET /:code/stats`;
  `AppState` ganha `analytics_tx`, `sink: Arc<dyn AnalyticsSink>` e `admin_token`.
- `src/main.rs` — cria o canal, sobe o worker, lê `QUARK_ADMIN_TOKEN`.

## 11. Testes

- **Agregação (unit, pura):** aplicar uma sequência de `ClickEvent` a `Aggregates`
  produz total/por-dia/por-país/por-device corretos.
- **Invariante sagrado (o principal):** o handler de redirect responde **302**
  sem bloquear nem falhar **com a fila de analytics cheia / worker parado**
  (drop-on-full comprovado).
- **Retenção:** após >N eventos num id, o `events` guarda exatamente os últimos N.
- **Endpoint:** sem token → 401; com token certo → 200 com os agregados; token
  não configurado no processo → 404.
- **Geo:** `country` é populado a partir do header `CF-IPCountry` quando presente.

## 12. Compatibilidade / riscos

- **Env compartilhado:** o sink embutido reusa o mesmo env LMDB (um `/data`, um
  mmap). Abrir o env duas vezes no mesmo processo falha no LMDB — a implementação
  deve compartilhar o `Env` (não reabrir o path).
- **Ordering do worker vs shutdown:** no shutdown, eventos ainda na fila podem se
  perder (best-effort, aceitável). Flush no drop é melhoria futura.
- **`StoreError` reusado** pelo sink por ora (mesma pega do Tijolo 1); generaliza
  quando o ClickHouse entrar.
- **Cardinalidade de `per_day`:** cresce 1 chave/dia por link — aceitável;
  compactação/rollup é problema de escala futura.
