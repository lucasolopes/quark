[English](ROADMAP.md) · **Português**

# Roadmap do quark

## Estado atual

Produto OSS de operador único essencialmente completo: **API + painel web**, sob
**licença AGPL-3.0** com **CLA** pra contribuições. Núcleo de binário único e
zero-dependências por padrão; backends de rede opt-in por env (escolhidos no
startup, sem feature flag de build). Testado (57 testes de lib + 29 de API +
suíte de integração gated Postgres/Valkey/ClickHouse, incl. busca + 34 testes de frontend) e
benchmarkado (permute ~264M ops/s; redirect ~7,9µs in-process; em produção
escalou linear até 1k VUs, gargalo medido = geografia/RTT, não o servidor).

## Feito

- **Núcleo (v0.1):** criar + redirecionar + alias customizado + expiração (TTL). O short-code
  é uma permutação Feistel/ARX calibrada (`ROUNDS=4`); códigos são **calculados, não
  armazenados** (store chaveado por `u64`).
- **Arquitetura plugável**: traits `Store` / `CacheTier` / `AnalyticsSink`:
  - **L2 Valkey** (`QUARK_VALKEY_URL`): cache compartilhado, circuit-breaker + timeout, fail-open.
  - **Postgres** (`QUARK_DATABASE_URL`): store relacional multi-nó (sequência de id atômica).
  - **ClickHouse** (`QUARK_CLICKHOUSE_URL`): sink de analytics OLAP (analytics-only).
- **Analytics de cliques**: captura fire-and-forget no 302 (~180ns) → worker → sink;
  `GET /:code/stats` (agregados + últimos N eventos).
- **Observabilidade**: log de acesso JSON por request, opt-in (`QUARK_ACCESS_LOG`).
- **Edge/CDN**: `Cache-Control` no redirect respeitando o TTL (guia em `docs/EDGE.md`).
- **Escala horizontal**: réplicas stateless sobre Postgres compartilhado; `QUARK_NODE_ID`
  particiona o espaço de id no LMDB (guarda defensiva). Doc: `docs/SCALING.md`.
- **Proteção contra abuso** (só no `POST /`): rate-limit por IP (`QUARK_RATELIMIT_PER_MIN`,
  memória/Valkey, fail-open), blocklist de destino no banco (`/admin/blocklist`, cache L1/L2),
  guarda embutida contra rede interna/loop (`QUARK_BLOCK_PRIVATE`, default on).
- **API do painel**: `GET /admin/links` (lista keyset paginada), `DELETE`/`PATCH /admin/links/:code`,
  tudo sob `QUARK_ADMIN_TOKEN`. **Criar (`POST /`) exige o token quando `QUARK_ADMIN_TOKEN`
  está configurado** (senão continua público). CORS opt-in via `QUARK_CORS_ORIGINS`.
- **Painel web (SPA)**: `web/` (React + Vite + shadcn/ui + TanStack + Recharts), deploy
  separado (build estático), binário API-only. Login por token → Links (CRUD, busca,
  copiar, **QR code**) → Stats por link (gráficos) → Blocklist. UI/UX seguindo
  heurísticas de Nielsen.
- **Busca server-side (Postgres)**: `GET /admin/links?q=` faz `ILIKE` em url+alias
  (keyset paginado, curingas escapados). Recurso do Postgres; o LMDB responde `501` e o
  painel cai pro filtro **client-side** (debounce ~300ms, fallback automático). Estado de
  erro distinto do "nada encontrado".
- **Licença + contribuições**: núcleo **AGPL-3.0-only**; `CLA.md` (license-grant) +
  `CONTRIBUTING.md` + bot do CLA (GitHub Action). Multi-tenancy/cloud fica proprietária, à parte.
- **`docker-compose.yml`**: stack full (quark + Postgres + Valkey + ClickHouse) pra dev/self-host.
- **Encaminhamento de conversão (#14)**: pixels GA4/Meta CAPI a nível de instância, encaminhados
  async pelo worker de analytics (nunca no caminho quente do redirect), fail-open. Painel: `/pixels`.
  Doc: `docs/CONVERSION-FORWARDING.PT_BR.md`.

## Próximo

- **Contas + painel multi-usuário**: é **fase cloud** (multi-tenant, proprietária). O OSS
  fica em conta única (operador). Merece brainstorming próprio quando for a hora.
- **Deploy da versão completa na VPS**: API + painel ainda não estão em produção
  (`quark.meuchat.ai` roda versão antiga); subir via Coolify.

## Backlog

- **Domínios customizados**: `meudominio.com/abc`.

## Restrições de design (conscientes)

- **Um binário puro (LMDB, sem banco) é single-node, por design**: não é limitação a remover.
  Escalar = réplicas stateless sobre Postgres compartilhado (`docs/SCALING.md`).
- **Proteção contra abuso** roda só no `POST /`; o redirect (caminho quente) não paga nada.
- **Criar link é público quando não há `QUARK_ADMIN_TOKEN`** (shortener aberto zero-config);
  configurar o token tranca a criação pro operador.

## Parqueado (futuro, não planejado)

- **Cloud full-edge na Cloudflare Workers**: direção da versão cloud (permute compila WASM;
  Store vira KV/D1/Durable Objects). Parqueado até virar prioridade.
- **Proxy shared-nothing** (multi-nó LMDB sem banco): não planejado; Postgres já cobre multi-nó.

## Notas

- Anti-abuso, escala horizontal, analytics e o painel **foram entregues** (não são mais futuros).
- A CI no GitHub tem um job Rust (com serviços valkey+postgres+clickhouse pros testes gated) e
  um job `web` (lint/typecheck/test/build do frontend). O CLA é coletado por bot em cada PR.
