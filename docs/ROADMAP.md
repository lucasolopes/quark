# Roadmap do quark

## Estado atual

Núcleo em produção + **arquitetura plugável completa**. Único binário e zero-dependências
por padrão; backends de rede opt-in por variável de ambiente, escolhidos no startup, sem
feature flags de build. Testado (53 testes de lib + 18 de API + suíte de integração gated
para Postgres/Valkey/ClickHouse) e benchmarkado (permute ~264M ops/s; redirect ~7,9µs
in-process; em produção escalou linear até 1k VUs, gargalo medido = geografia/RTT, não o
servidor).

## Feito

- **Núcleo (v0.1):** criar + redirecionar + alias customizado + expiração (TTL). O short-code
  é uma permutação Feistel/ARX calibrada (`ROUNDS=4`); códigos são **calculados, não
  armazenados** (store chaveado por `u64`).
- **Arquitetura plugável** — traits `Store` / `CacheTier` / `AnalyticsSink`:
  - **L2 Valkey** (`QUARK_VALKEY_URL`) — cache compartilhado entre réplicas, com
    circuit-breaker + timeout de 100ms, fail-open (Valkey caído nunca trava o redirect).
  - **Postgres** (`QUARK_DATABASE_URL`) — store relacional multi-nó (sequência de id atômica).
  - **ClickHouse** (`QUARK_CLICKHOUSE_URL`) — sink de analytics OLAP (analytics-only, nunca store).
- **Analytics de cliques** — captura fire-and-forget no 302 (custo medido ~180ns, ~2,3% do
  handler) → worker de fundo (batch) → sink; `GET /:code/stats` (agregados + últimos N eventos),
  protegido por `QUARK_ADMIN_TOKEN`.
- **Observabilidade** — log de acesso JSON por request, **opt-in** (`QUARK_ACCESS_LOG`), fora do
  caminho quente por padrão.
- **Edge/CDN** — `Cache-Control` no redirect respeitando o TTL do link (guia em `docs/EDGE.md`).
- **Escala horizontal** — réplicas stateless sobre Postgres compartilhado; `QUARK_NODE_ID`
  particiona o espaço de id no LMDB (guarda defensiva contra colisão de código). Doc:
  `docs/SCALING.md`.
- **Proteção contra abuso** — só no `POST /`: rate-limit por IP (memória ou Valkey, fail-open,
  opt-in via `QUARK_RATELIMIT_PER_MIN`), blocklist de destino **no banco** (match
  domínio+subdomínio, cache snapshot L1/L2, gerida por `GET/POST/DELETE /admin/blocklist`), e
  guarda embutida contra rede interna/loop (default on, `QUARK_BLOCK_PRIVATE=0` desliga).

## Próximo

- **Contas + painel web** — login + UI pra gerenciar links. É o que faz o quark deixar de ser
  só infra e virar produto (concorrente dos SaaS). Modelo de produto travado: **versão OSS =
  conta única; versão cloud = multi-tenant**. O painel consome os endpoints `/admin/*` já
  existentes. Merece brainstorming próprio (backend de auth + modelo de usuário + frontend).

## Backlog

- **Domínios customizados + QR code.**
- **Deploy da versão nova na VPS:** `quark.meuchat.ai` ainda roda uma versão anterior aos
  tijolos 3–7; subir via Coolify quando quiser.

## Restrições de design (conscientes)

- **Um binário puro (LMDB, sem banco) é single-node, por design.** Não é uma limitação a ser
  removida: é uma escolha. Escalar horizontalmente = rodar réplicas stateless sobre **Postgres
  compartilhado** (formato 2 em `docs/SCALING.md`). O `QUARK_NODE_ID` existe só como guarda
  defensiva contra colisão de código, não pra transformar o LMDB em multi-nó.
  - *Nota, não plano:* como o `QUARK_NODE_ID` fica nos bits altos do id, seria teoricamente
    possível um nó decodificar o código e fazer proxy pro nó dono ("shared-nothing", sem
    banco). Fica registrado como curiosidade — **não está planejado**; a restrição single-node
    do binário puro é deliberada e o Postgres já cobre multi-nó.
- **Proteção contra abuso** roda só no `POST /`; o redirect é caminho quente e não paga nada.

## Notas

- Anti-abuso, escala horizontal e analytics **não são mais diferidos** — foram entregues.
- A CI no GitHub sobe serviços valkey + postgres + clickhouse para os testes de integração gated.
