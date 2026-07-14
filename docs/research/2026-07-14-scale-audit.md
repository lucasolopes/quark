# quark — auditoria de escala e durabilidade (2026-07-14)

Investigação profunda, subsistema por subsistema, com evidência no código (`file:line`), cruzada com as melhores práticas da indústria (ver `2026-07-14-scale-durability-best-practices.md`). Responde a duas perguntas do dono: (1) não reinventar fila/db no binário; (2) escalar TODAS as features, não só o redirect.

## Veredito de uma linha

O quark escala de verdade **só na configuração Postgres + Valkey + ClickHouse**. Mesmo aí sobram (a) duas janelas de consistência-eventual limitadas e baratas de fechar, e (b) um antipadrão real: filas best-effort in-process para webhooks, analytics e pixels.

## Matriz por subsistema

| Subsistema | LMDB (default) | Postgres/Valkey/ClickHouse | Status real |
|---|---|---|---|
| Redirect (hot path) | ok single-node | computado, cache tier | **escala** |
| Alocação de ID | contador por nó + prefixo node_id (só com node_id único; teto 256 nós × 4.29B) | sequência compartilhada (coordenada) | **escala** (PG); ok multi-nó com ressalva (LMDB) |
| Rate limit | memória = por nó → limite real vira **N×** | Valkey = contador atômico global | **escala só em modo Valkey** |
| Blocklist | snapshot + TTL por nó | idem (+ L2 Valkey) | eventual ≤ TTL (60s) entre nós |
| Cache | L1 por nó; **store não compartilhado** | L1 por nó + L2 Valkey; sem invalidação cross-node | eventual ≤ 60s no patch/delete; LMDB não é coerente multi-nó |
| Analytics (agregação) | RMW de blob, **por nó** (subcontagem) | RMW de blob sob `pg_advisory_xact_lock` — correto mas **hotspot por link** | **ClickHouse**: append-only + agg-on-read = **escala** |
| Ingestão de clique | `try_send` **descarta no cheio** (canal 10k), buffer em RAM perdido no crash | igual em todos os sinks | **best-effort / lossy** em todos |
| Entrega de webhook | `mpsc` in-process, descarta no cheio (1024), sem durabilidade/retry-durável/DLQ | igual em todos os backends | **best-effort / single-node** |
| Forward de pixel | best-effort no worker, sem retry, **sem chave de dedup** | igual | **best-effort / lossy** |

## Os gaps, ranqueados, com o fix certo

### 1. Entrega de webhook — o antipadrão principal (crítico)
Evidência: `src/webhooks/delivery.rs:22,60,245-293`, emit pós-commit não-transacional em `src/api.rs:420`. Um `mpsc` de 1024 que descarta no cheio; nada do evento/estado de entrega é persistido; retry vive na stack (~600ms), some no restart; sem DLQ; `webhook-id` aleatório não-persistido (não dedup entre nós/restart); entrega serial → um endpoint lento (~11s/evento) trava todos e derruba o canal.
**Fix:** **outbox transacional no Postgres** (evento gravado na MESMA tx da mutação do link) + relay com **`SELECT ... FOR UPDATE SKIP LOCKED`** (N réplicas disjuntas) + **retry persistido + dead-letter** + **idempotency key estável** (event_id + sub_id) no header em toda tentativa/nó. Sem dependência nova (é o Postgres que já existe).

### 2. Ingestão de analytics lossy + agregação hotspot (alto)
Evidência: `src/api.rs:885` (`try_send` descartado), `src/analytics/mod.rs:304,319-322` (buffer em RAM), `src/store/postgres.rs:711-782` (RMW de blob sob advisory-lock por link).
**Fix:** (a) trocar o RMW de blob por **incrementos atômicos** (`INSERT ... ON CONFLICT DO UPDATE SET count = count + n` numa tabela `(id, dimensão, chave, dia)`) **ou** recomendar **ClickHouse** como sink de analytics multi-nó (append-only, já correto e escalável). (b) para não perder clique sob carga, log durável de ingestão (ou assumir explicitamente o at-most-once como amostragem sob pico). ClickHouse já resolve a agregação; a ingestão continua best-effort até ter um log durável.

### 3. Pixel sem chave de dedup (bloqueia durabilidade futura) (alto)
Evidência: `src/pixel.rs:129-193` (sem `transaction_id`/`event_id`), `src/analytics/mod.rs:380-399` (sem retry), `docs/CONVERSION-FORWARDING.md:101-104`.
**Fix:** emitir **`event_id` (Meta)** e **`transaction_id` (GA4)** estáveis por clique **primeiro** — sem isso, qualquer retry futuro dobra conversão. Depois, outbox/retry como no #1.

### 4. Coerência de cache + blocklist entre nós (médio) — **um fix resolve os dois**
Evidência: `src/cache/mod.rs:172-183` (L2 DEL não alcança L1 já populado de outro nó), `src/abuse/blocklist.rs:15,47-64` (propagação ≤ TTL).
**Fix:** **um canal Valkey pub/sub de invalidação**: no patch/delete/block, publica `invalidate`; cada réplica assina e zera o L1/snapshot na hora. Vira ≤60s → ~instantâneo. Reaproveita o L2/DEL que já existem.

### 5. Rate limit N× em modo memória (médio)
**Fix:** recusar subir multi-nó sem `QUARK_VALKEY_URL` (o código já loga a diferença em `src/main.rs:126-132`, só não força).

### 6. LMDB é single-node de fato + node_id não é validado único (documentar)
Réplicas LMDB não compartilham dados (arquivos separados); o node_id só evita colisão de ID, não torna o store compartilhado. Teto global de 2^40 links no caminho Postgres (`src/api.rs:440`).
**Fix:** documentar que multi-nó real exige Postgres; validar unicidade do node_id.

## Princípio validado (pesquisa)

O padrão de seam plugável (embedded default + serviço real opt-in) é o certo. Embutir é adequado para **durabilidade local** (moka, canal de analytics). Vira antipadrão quando um componente in-process **finge** uma garantia de durabilidade/coordenação **entre processos** — exatamente o que a fila de webhook em memória faz. Mover pro outbox drenado por Postgres (`SKIP LOCKED`) corrige **sem dependência nova**.

## Ordem recomendada para fechar

1. **Pub/sub de invalidação (cache + blocklist)** — 1 canal, fecha 2 gaps, barato, alto valor de correção.
2. **Chaves de dedup no pixel** (event_id/transaction_id) — pré-requisito barato pra qualquer durabilidade futura.
3. **Outbox de webhook (Postgres + SKIP LOCKED + retry/DLQ + idempotency)** — o antipadrão principal.
4. **Agregação por incremento atômico / ClickHouse recomendado** — tira o hotspot por link.
5. **Enforce Valkey no multi-nó** + validar node_id + documentar LMDB single-node.
