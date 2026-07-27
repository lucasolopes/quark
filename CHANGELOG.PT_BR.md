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

## [0.4.1] - 2026-07-27

### Adicionado

- **Exclusão de workspace.** `DELETE /admin/tenants/{id}` remove o workspace e
  tudo que pertence a ele, numa transação só, mais os cliques no ClickHouse e o
  realm no Keycloak. Só o `Owner` exclui, o último workspace não pode ser
  excluído, e um tenant do qual quem chama não é membro responde `404` e não
  `403`, para o endpoint não servir de enumeração de workspaces. O painel exige
  que o slug seja digitado antes de prosseguir. Excluir libera o slug para
  reuso. O `docs/WORKSPACES.PT_BR.md` detalha o que some na hora e o que some
  depois.
- **A criação de workspace explica a espera.** A requisição provisiona realm,
  client, mapper e usuário no Keycloak, então é legitimamente lenta. O painel
  passa a dizer que um login está sendo preparado e, passado um limite,
  acrescenta que está demorando mais que o normal e que recarregar é seguro.
- `disabled_reason` na representação do webhook em `GET /admin/webhooks`, que
  permite ao painel distinguir uma subscription que o usuário pausou de uma que
  o sistema desativou.

### Corrigido

- **A URL de destino dos webhooks não vai mais para o log.** No Discord, no
  Slack, no Telegram e nos conectores genéricos o token fica no path da URL, ou
  seja, a URL é a credencial. Ela saía inteira em dez pontos de log. O campo
  agora é um `WebhookUrl` cujo `Display` imprime só host e porta, e o `reqwest`
  não aceita esse tipo sem um `expose()` explícito, então reintroduzir o
  vazamento passa a ser erro de compilação em vez de descuido de revisão. O
  segredo de assinatura da subscription também era legível pelo `Debug` do
  struct, e deixou de ser.
- **Um destino de webhook morto não é mais retentado para sempre.** `404` e
  `410` significam que o destino não existe mais, mas toda resposta não-2xx era
  tratada igual, então um endpoint removido gastava o orçamento inteiro de
  tentativas a cada evento, indefinidamente. Esses dois status passam a ter uma
  tentativa de confirmação, e o destino que falha nela é desativado com o motivo
  registrado e visível no painel. `429`, `5xx`, timeout e erro de transporte
  mantêm o backoff atual. `400` e `422` ficaram de fora de propósito: costumam
  indicar que o nosso payload está errado, e desativar a integração do cliente
  por bug nosso é o pior desfecho possível.
- **Reconectar o Slack por cima de uma subscription desativada agora a
  reativa.** O merge do OAuth herdava `active: false` e o motivo antigo, então a
  correção óbvia (reconectar) parecia funcionar e não entregava nada.
- **Os eventos de log do `main.rs` voltaram a ter campos.** Oito pontos montavam
  JSON à mão dentro da macro, o que sob `QUARK_LOG_FORMAT=json` produzia um
  objeto serializado escapado dentro de `message`: os valores viravam texto em
  vez de campos consultáveis. Seis deles logavam erro em `info`, nível que
  nenhum alerta baseado em severidade jamais pegaria.

## [0.4.0] - 2026-07-26

### Alterado

- **BREAKING: `QUARK_ACCESS_LOG` deixou de existir.** O log de acesso por
  requisição agora vem do `TraceLayer` do tower-http e sai em `DEBUG`, ou seja,
  fica desligado sob o filtro `info` padrão e liga com
  `RUST_LOG=tower_http=debug`. Um deploy que ainda define `QUARK_ACCESS_LOG` não
  quebra, mas para de produzir log de acesso. A nova `QUARK_LOG_FORMAT=json`
  passa todo evento de log a sair como um objeto JSON por linha, que é o que um
  pipeline de logs espera.
- Os erros passam a ser tipados com `thiserror` no crate inteiro, e o log vai
  por `tracing` no lugar de `eprintln!`. Nada muda no contrato HTTP: os handlers
  devolvem os mesmos status e os mesmos corpos curtos de erro.
- As chaves de assinatura ficam em `secrecy::SecretBox`, então são zeradas no
  drop e não podem ser impressas por acidente.
- Dependências: axum 0.7 para 0.8, heed 0.20 para 0.22, redis 0.27 para 1.x,
  sqlx 0.8 para 0.9. Nenhuma delas muda formato em disco, migração ou formato
  de fio.

### Corrigido

- **SSRF: `[::127.0.0.1]` era aceito como destino.** A checagem de endereço
  interno existia em duas cópias que tinham divergido, e a da criação de link
  não rejeitava endereços IPv6 compatíveis com IPv4. Agora existe um único
  `is_internal_ip`, que cobre IPv6 mapeado e compatível com IPv4, CGNAT
  (100.64/10), `0.0.0.0/8`, multicast e as faixas de documentação.
- O `state` do login OIDC passa a ser comparado em tempo constante.
- O `POST /admin/logout` usa o guard de CSRF compartilhado no lugar da checagem
  própria de header, então aceita as mesmas provas que todo outro endpoint que
  muda estado.
- Um evento de clique descartado (canal de analytics cheio) passa a ser contado
  e logado em vez de sumir, deixando a saturação visível. O contador só roda no
  caminho de descarte, então um redirect saudável não paga nada por ele.
- A configuração de OIDC lê cada variável obrigatória uma vez e carrega o valor,
  em vez de validar a variável e reler depois.

### Segurança

- `unsafe_code = "deny"` e uma política de clippy (`unwrap_used`, `expect_used`,
  `panic`, `await_holding_lock`, `let_underscore_future`) são obrigatórias no
  CI. Os 28 `expect()` que restam em `src/` carregam, cada um, uma justificativa
  escrita.

## [0.3.1] - 2026-07-25

### Corrigido
- O login OIDC não derruba mais o servidor: o upgrade do `jsonwebtoken` 10 saiu sem backend de crypto, então validar qualquer id_token panicava e reiniciava o processo. A feature `rust_crypto` agora está fixada e um teste canário exercita uma operação JWT real no CI, para essa classe de regressão quebrar o build em vez da produção.

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

[Não lançado]: https://github.com/lucasolopes/quark/compare/v0.4.1...HEAD
[0.4.1]: https://github.com/lucasolopes/quark/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/lucasolopes/quark/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/lucasolopes/quark/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/lucasolopes/quark/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/lucasolopes/quark/releases/tag/v0.2.0
