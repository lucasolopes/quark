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

- **Webhooks (#1):** eventos HTTP de saída assinados em `link.created/updated/deleted/expired/clicked`,
  assinatura HMAC Standard Webhooks, entrega best-effort (fila → worker → retry com backoff/jitter,
  guardada contra SSRF), assinaturas gerenciadas no painel ou via `/admin/webhooks`. É a base
  pro #6 (Slack/Discord/Telegram) e o #10 (n8n/Zapier). Doc: `docs/WEBHOOKS.PT_BR.md`.
- **Canais de notificação (#6):** Slack/Discord/Telegram como um `kind` na assinatura de webhook
  (construído sobre o #1); mensagem em texto plano, não assinada, no formato de cada canal
  (Slack/Telegram `{"text": ...}`, Discord `{"content": ...}`), autenticada pela URL secreta do
  canal em vez de HMAC. Doc: `docs/WEBHOOKS.PT_BR.md` ("Canais de notificação").
- **Núcleo (v0.1):** criar + redirecionar + alias customizado + expiração (TTL). O short-code
  é uma permutação Feistel/ARX calibrada (`ROUNDS=4`); códigos são **calculados, não
  armazenados** (store chaveado por `u64`).
- **Expiração por máximo de visitas (#11):** um link expira por TTL ou por um número máximo de visitas, o que vier primeiro.
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
  tags, copiar, **QR code**) → Stats por link (gráficos) → Blocklist. UI/UX seguindo
  heurísticas de Nielsen.
- **Tags (#7)**: links carregam tags normalizadas (`Record.tags`: aparadas,
  minúsculas, deduplicadas, com limite) pra organizar; a lista de links filtra
  por tag (`GET /admin/links?tag=`), e `GET /admin/tags` lista o conjunto
  distinto pro painel. Um dashboard de stats agregado cross-tag fica como
  follow-up.
- **Busca server-side (Postgres)**: `GET /admin/links?q=` faz `ILIKE` em url+alias
  (keyset paginado, curingas escapados). Recurso do Postgres; o LMDB responde `501` e o
  painel cai pro filtro **client-side** (debounce ~300ms, fallback automático). Estado de
  erro distinto do "nada encontrado".
- **Licença + contribuições**: núcleo **AGPL-3.0-only**; `CLA.md` (license-grant) +
  `CONTRIBUTING.md` + bot do CLA (GitHub Action). Multi-tenancy/cloud fica proprietária, à parte.
- **`docker-compose.yml`**: stack full (quark + Postgres + Valkey + ClickHouse) pra dev/self-host.
- **Importador (#4)**: `POST /admin/import` cria links em lote a partir de um CSV ou JSON exportado
  (Bitly, Kutt, YOURLS, genérico), relatório de sucesso parcial por linha, mais uma aba "Import" no
  painel web. Doc: [`docs/IMPORT.PT_BR.md`](IMPORT.PT_BR.md).
- **Builder de UTM + templates**: seção colapsável de UTM no diálogo de criar link, com
  prévia ao vivo do destino e templates nomeados salvos localmente (`localStorage`).
- **#9 Tokens de API com escopos + quota**: tokens nomeados (`links_read`, `links_write`, `blocklist`, `webhooks`, `analytics`, `full`) com limite de requisições opcional por token, gerenciados em `/admin/tokens` e na página **Tokens de API** do painel; o `QUARK_ADMIN_TOKEN` do env continua se comportando como `full`, sem mudanças. Doc: `docs/API-TOKENS.PT_BR.md`.
- **Regras de redirecionamento (#12)**: regras por link de geo/dispositivo (primeira que combina vence, `url` continua o padrão), editor no painel nos diálogos de criar/editar. Doc: `docs/REDIRECT-RULES.PT_BR.md`.

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
