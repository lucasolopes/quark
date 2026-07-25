[English](CHANGELOG.md) · **Português**

# Changelog

Todas as mudanças relevantes do projeto ficam registradas aqui. O formato segue
o [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/).

O versionamento é SemVer com semântica 0.x, a convenção do Cargo:

- `0.MINOR.0` é uma release com quebra de compatibilidade. Pode mudar a API
  HTTP, renomear ou remover uma variável `QUARK_*`, ou mudar o formato em
  disco.
- `0.MINOR.PATCH` é funcionalidade compatível e correção de bug.
- `0.MINOR.0-rc.N` é pré-release e nunca recebe a tag `latest` da imagem.

O contrato público coberto por esses números é: a API HTTP (`/`, `/:code`,
`/:code/stats`, `/admin/*`), as variáveis `QUARK_*`, o formato do LMDB em disco
e as migrações do Postgres, e o payload e a assinatura dos webhooks. A
superfície de biblioteca em `src/lib.rs`, o HTML do painel admin e o layout das
tabelas do ClickHouse não estão cobertos.

## [Não lançado]

## [0.3.0] - 2026-07-25

### Adicionado
- Painel totalmente responsivo (mobile, tablet, desktop): drawer de navegação com hambúrguer em telas pequenas, dialogs de criar/editar link em tela cheia no celular, reflow de cada tela até 360px de largura e alvos de toque de 44px nos controles principais.
- Script local de QA responsivo (`web/scripts/responsive-qa.mjs`): varre todas as telas em 4 breakpoints e nos dois temas, falhando em qualquer overflow horizontal.

### Alterado
- Deploys de produção agora são orientados a release: só tags de versão disparam deploy, por um único workflow de release.
- Upgrades maiores de dependências: axum 0.8, cliente ClickHouse 0.15, chacha20poly1305 0.11, redis 1.4, além de bumps do toolchain React/Vite.

### Corrigido
- Os gráficos de estatísticas não quebram mais quando o label do tooltip não é string.

### Segurança
- A validação do id_token OIDC agora exige as claims `exp`, `iss` e `aud`.

## [0.2.0] - 2026-07-24

Primeira tag e primeira imagem de container publicada. Tudo abaixo já estava
na `main` desde o começo do projeto; esta entrada marca o ponto em que ele
virou instalável.

### Adicionado
- Códigos curtos calculados por uma rede Feistel com função de rodada ARX,
  uma bijeção sobre o espaço de ids, sem índice de códigos guardado em disco.
- Armazenamento plugável: LMDB embutido (padrão, sem dependência externa) ou
  Postgres para uma implantação multi-nó com banco compartilhado.
- Cache plugável: em processo por padrão, com camada L2 opcional em Valkey e
  invalidação entre nós via pub/sub do Valkey.
- Analytics plugável: coletor embutido por padrão, ou ClickHouse como backend
  de analytics OLAP; `GET /:code/stats` para agregados e eventos recentes.
- Login OIDC (Authorization Code + PKCE) como alternativa ao token de admin,
  com sessões opacas e revogáveis mantidas no servidor.
- Webhooks de saída assinados seguindo a spec Standard Webhooks, nos eventos
  `link.created/updated/deleted/expired/clicked/broken/recovered`; uma fila de
  saída (outbox) durável no Postgres com retentativa, backoff e dead-letter,
  entrega best-effort no LMDB; canais de notificação para Slack/Discord/
  Telegram construídos sobre o mesmo modelo de assinatura.
- Tokens de API com escopo (`links_read`, `links_write`, `webhooks`,
  `analytics`, `full`) e limite de taxa opcional por token.
- Regras de redirecionamento: segmentação por geo/dispositivo por link, a
  primeira regra que casar vence.
- Testes A/B: variantes de link com peso, com estatísticas de clique por
  variante.
- Deep linking: hospeda os arquivos `apple-app-site-association` (iOS) e
  `assetlinks.json` (Android), além do redirecionamento sensível ao
  dispositivo para um destino de app.
- Links protegidos por senha (argon2id), expiração por número máximo de
  visitas com URL de fallback opcional, e monitoramento de link quebrado com
  notificação via webhook nas transições de status.
- Encaminhamento de conversão para GA4 e Meta CAPI, disparado fora do caminho
  crítico do redirecionamento.
- Importador de exportações CSV/JSON do Bitly, Kutt, YOURLS e um formato
  genérico, com relatório de sucesso parcial por linha.
- Tags, um montador de UTM com templates salvos localmente, e busca no lado
  do servidor no Postgres (com fallback no lado do cliente no LMDB).
- Proteção contra abuso na criação de links: limite de taxa por IP e uma
  proteção embutida contra alvos de rede interna/loopback (SSRF).
- Painel admin (React, Vite, shadcn/ui, TanStack, Recharts): CRUD de links,
  busca, tags, QR code, estatísticas por link, gestão de tokens de API.
- `docker-compose.yml` para uma stack local completa (quark, Postgres,
  Valkey, ClickHouse).
- `quark --version` e um cabeçalho `X-Quark-Version` em `GET /health`.

### Segurança
- Núcleo AGPL-3.0-only com um CLA coletado em cada pull request.
- Relato privado de vulnerabilidade e política de segurança escrita.

[Não lançado]: https://github.com/lucasolopes/quark/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/lucasolopes/quark/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/lucasolopes/quark/releases/tag/v0.2.0
