# Central de integracoes, Fase 3: connector_id + health

Design da Fase 3 da LUC-87. As Fases 1 e 2 estao no ar (catalogo com view
dedicada por integracao; Slack "Add to Slack" via OAuth). Esta fase resolve tres
dores concretas que sobraram, sem reescrever os subsistemas de webhooks, pixels e
sheets.

A direcao escolhida e **aumentar in-place**: adicionar `connector_id` e campos de
health aos tipos ja persistidos, generalizando o modelo que o Sheets ja usa
(`last_sync` + `last_status`). Nao ha tabela `Connection` generica nesta fase (a
opcao de fachada unica da spec original fica adiada; o custo nao se paga contra as
dores atuais).

## Dores que esta fase resolve

1. **Zapier/Make/n8n indistinguiveis.** Os tres compartilham `kind: "generic"`,
   entao `useConnectedIds` acende os tres juntos assim que qualquer webhook
   generico existe (`web/src/lib/connectors.ts:96-103`,
   `web/src/routes/ExtensionDetail.tsx:247-251`).
2. **Sem health de entrega.** So o Sheets mostra "ultima sincronizacao / erro". Um
   webhook ou pixel conectado nao mostra se a ultima entrega deu certo.
3. **Match do Slack fragil a rename.** O dedup de reinstalacao da Fase 2 casa pelo
   nome do canal (`label`); renomear o canal quebra o match e duplica a conexao.

## Escopo

Dentro:

- `connector_id` opcional no `WebhookSubscription` para desambiguar os genericos.
- Health passivo de webhook (ultima entrega + status), fora do hot path.
- Health passivo de pixel (ultimo forward + status).
- Match do Slack por `channel_id` (a prova de rename).
- Superficie: API rows + tipos TS + render de health nos paineis.

Fora:

- Tabela `Connection` generica / fachada unica (adiada).
- Conectores novos (Notion/TikTok/LinkedIn), que sao a Fase 4.
- Polling ativo de health. So sinais passivos (resultado da ultima entrega/sync).
- Health no hot path de clique (`link.clicked`), por custo por requisicao.

## Modelo de dados

### WebhookSubscription (`src/webhooks/mod.rs:119`)

Tres campos novos, todos `#[serde(default)]` para back-compat de blob:

```
connector_id: Option<String>   // id do catalogo: "zapier"|"make"|"n8n"|"slack"...
external_id: Option<String>    // id estavel do lado do provedor (Slack: channel_id)
last_delivery_at: Option<u64>
last_delivery_status: DeliveryStatus
```

`DeliveryStatus` espelha o `SyncStatus` do Sheets (`src/sheets/mod.rs:72`):

```
enum DeliveryStatus { Never, Ok, Error(String) }   // tagged, default Never
```

`external_id` e generico de proposito (nao "slack_channel_id") para reuso por
conectores futuros que tenham um id estavel de destino.

### PixelConfig (`src/pixel.rs:47`)

Pixels ja se desambiguam por `provider`, entao **nao** ganham `connector_id`. So
health:

```
last_forward_at: Option<u64>
last_forward_status: ForwardStatus   // mesmo shape de DeliveryStatus, default Never
```

## Health passivo: como e gravado

Regra unica: **nunca gravar health no caminho de `link.clicked`** (hot path de
redirect). Todo o resto grava best-effort (falha de gravacao e ignorada, nunca
bloqueia a entrega).

### Webhook

Novo metodo no trait `Store` (`src/store/mod.rs:322`):

```
async fn record_webhook_health(&self, tenant: TenantId, id: u64, at: u64, status: DeliveryStatus) -> Result<()>;
```

- **Postgres:** `UPDATE webhooks SET last_delivery_at=$1, last_delivery_status=$2
  WHERE id=$3 AND tenant_id=$4`. Update cirurgico, nao reescreve a linha inteira.
- **LMDB:** read-modify-write do blob dentro de uma txn de escrita.

Pontos de chamada:

- `deliver_one` (worker in-memory, `src/webhooks/delivery.rs:402`): grava apos o
  POST, **exceto** quando `event == EventType::LinkClicked`.
- `deliver_claimed` (relay Postgres, `src/webhooks/delivery.rs:569`): idem, apos
  `mark_delivered`/`mark_retry`/`mark_dead`.
- Handler `/test` (`src/api/webhooks_api.rs:526`): sempre grava (nunca e clique).

O worker in-memory precisa do `tenant` junto do `id` da subscription para chamar
`record_webhook_health`. Se o tipo de trabalho enfileirado hoje nao carrega o
tenant, o plano inclui o wiring minimo para propagar (verificar na
implementacao).

### Pixel

Metodo analogo `record_pixel_health(tenant, id, at, status)`. Gravado apos o
forward de conversao (`src/pixel.rs`), best-effort. O forward roda fora do path
principal de redirect, mas ainda assim a gravacao e best-effort e nunca bloqueia.

## Match do Slack por channel_id (`src/slack.rs`, `src/api/slack.rs`)

- `src/slack.rs`: o parser da resposta do OAuth passa a extrair
  `incoming_webhook.channel_id` (alem do `channel` que ja lemos).
- `src/api/slack.rs` (`slack_callback`, ~`:120-152`): grava `external_id =
  channel_id` e `connector_id = "slack"` na subscription. O dedup passa a casar na
  ordem: `external_id == channel_id` (a prova de rename) -> `label` -> `url`
  (legacy). Quando acha, atualiza a linha in-place (nova url + label + reafirma o
  external_id) em vez de inserir.

## connector_id nos webhooks genericos

- Create API (`src/api/webhooks_api.rs:205`): `CreateReq` aceita `connector_id`
  opcional. A view dedicada de cada conector no front ja sabe qual e (o `:id` da
  rota) e envia.
- `useConnectedIds` (`web/src/lib/connectors.ts:104`): quando a subscription tem
  `connector_id`, casa por ele (atribuicao precisa). Sem `connector_id` (linhas
  legacy), mantem o comportamento atual por `kind` (o trio generico acende junto).
  Assim funciona resetando o prod ou nao.
- `WebhookPanel` (`ExtensionDetail.tsx:239`): filtra por `connector_id` quando
  presente, caindo para `kind` no legacy.

## Superficie API + Front

- `WebhookRow` (`src/api/webhooks_api.rs:20`): + `connector_id`,
  `last_delivery_at`, `last_delivery_status`.
- `PixelRow` (`src/api/webhooks_api.rs:114`): + `last_forward_at`,
  `last_forward_status`.
- Tipos TS `Webhook`/`Pixel` (`web/src/lib/types.ts:166,212`): campos espelhados.
- `WebhookPanel`/`PixelPanel` (`ExtensionDetail.tsx`): linha de health espelhando
  o `SheetsPanel` (`:195-206`): "ultima entrega: 200, ha 3 min" no caso Ok, banner
  de erro com o detalhe no caso Error, nada no caso Never.

## Persistencia e constraints do projeto

- **Postgres** (`src/store/postgres.rs`): `ALTER TABLE webhooks ADD COLUMN IF NOT
  EXISTS` para `connector_id TEXT`, `external_id TEXT`, `last_delivery_at BIGINT`,
  `last_delivery_status JSONB`; `ALTER TABLE pixels ADD COLUMN IF NOT EXISTS` para
  `last_forward_at BIGINT`, `last_forward_status JSONB`. Atualizar `put_webhook`
  (`:1431`), `put_pixel`, `row_to_webhook` (`:367`), `row_to_pixel` (`:485`) e os
  `SELECT` de `list_webhooks`/`get_webhook`. Sem tabela nova, entao o TRUNCATE de
  teste (`:1055`) fica intocado.
- **LMDB** (`src/store/lmdb.rs`): de graca via `#[serde(default)]` no blob. Sem
  novo sub-DB, entao `MAX_DBS` (`:94`) fica intocado.
- Sem `CREATE INDEX CONCURRENTLY`. `codec.rs` e `permute.rs` intocados.

## Plano de testes (TDD)

- Serde: `DeliveryStatus`/`ForwardStatus` default `Never`; round-trip tagged; blob
  antigo (sem os campos novos) desserializa com os defaults.
- Store round-trip dos campos novos (LMDB sempre; Postgres gated em
  `QUARK_TEST_DATABASE_URL`, nao-superuser, `-j1`).
- `record_webhook_health` atualiza so a linha alvo e nao mexe nos outros campos.
- `deliver_one`/`deliver_claimed` gravam health para eventos nao-clique e **nao**
  gravam para `link.clicked`.
- Slack: reinstalar com o mesmo `channel_id` e nome diferente atualiza a
  subscription in-place (nao duplica); `external_id` e persistido.
- Front: `useConnectedIds` atribui certo dois genericos com `connector_id`
  distintos; paineis renderizam health nos tres estados.

## Riscos

- **Wiring do tenant no worker in-memory.** Se a fila de entrega nao carrega o
  tenant hoje, propagar isso e o unico ponto de acoplamento novo. Delimitado e
  coberto pelo plano.
- **Best-effort de verdade.** Gravar health nunca pode falhar a entrega nem
  atrasar o hot path. Todo `record_*_health` e fire-and-forget com erro logado, nao
  propagado.
