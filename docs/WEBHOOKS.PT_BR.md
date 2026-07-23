[English](WEBHOOKS.md) · **Português**

# Webhooks

Uma instância quark de operador único pode enviar eventos HTTP assinados para
um endpoint externo (Zapier, Make, n8n, Slack ou qualquer receptor
customizado). Como a entrega se comporta depende do backend:

- No backend **Postgres**, os eventos de ciclo de vida (`link.created`,
  `link.updated`, `link.deleted`) são entregues de forma **durável**: eles
  caem numa outbox no Postgres e um worker de relay com lease os entrega
  pelo menos uma vez, com retry persistido e uma fila de mortos
  (dead-letter). Veja [Entrega durável
  (Postgres)](#entrega-durável-postgres).
- `link.clicked` (e `link.expired`, que também dispara no caminho quente do
  redirect) continuam **best-effort** em todo backend: uma fila limitada em
  memória alimenta um worker que assina e envia via POST com retry, e um
  evento descartado numa fila cheia ou num restart é simplesmente perdido.
  Adicionar uma escrita síncrona no banco ao caminho do redirect derrubaria
  o propósito dele, então esses dois eventos são best-effort por decisão de
  projeto.
- No backend **LMDB** de nó único não há outbox; todo evento, incluindo os
  de ciclo de vida, anda pelo canal best-effort em memória.

A *configuração* da assinatura (URL, conjunto de eventos, segredo, flag de
ativo) é sempre durável, persistida no store (LMDB ou Postgres),
independente de como os eventos em si são entregues.

## Eventos

| Evento | Dispara quando |
|---|---|
| `link.created` | Um link é criado (`POST /`). |
| `link.updated` | Um link é editado (`PATCH /admin/links/:code`). |
| `link.deleted` | Um link é removido (`DELETE /admin/links/:code`). |
| `link.expired` | Um redirect resolve um link além do TTL (o caminho do `410 Gone`). Não há sweeper em background: a expiração é observada no acesso, igual ao resto do tratamento de TTL do quark. |
| `link.clicked` | Um redirect tem sucesso. Emitido pelo caminho assíncrono, nunca pelo caminho quente do 302, e só quando pelo menos uma assinatura ativa quer esse evento (uma flag atômica em cache mantém o custo zero no resto do tempo). |
| `link.threshold_reached` | Um link é clicado pelo menos `threshold` vezes dentro de uma janela fixa de `window_secs` segundos, conforme a regra de alerta do link. Dispara uma vez por janela (veja [Alertas de limiar de cliques](#alertas-de-limiar-de-cliques)). |

## Payload

Toda entrega tem o mesmo envelope:

```json
{
  "id": "evt_...",
  "type": "link.created",
  "timestamp": 1699999999,
  "data": {
    "code": "aB3xZ9k",
    "url": "https://example.com/dest",
    "alias": "promo",
    "expiry": 1700003599,
    "created": 1699990000
  }
}
```

- `id`: um id de evento aleatório, distinto por emissão.
- `type`: um dos nomes de evento acima.
- `timestamp`: unix seconds, quando o evento foi gerado.
- `data.alias` e `data.expiry` são omitidos (não enviados como `null`) quando
  o link não tem alias ou não tem TTL.

`link.clicked` carrega o mesmo contexto de clique já capturado para
analytics, além de `code`, `url` e `created`:

```json
{
  "id": "evt_...",
  "type": "link.clicked",
  "timestamp": 1699999999,
  "data": {
    "code": "aB3xZ9k",
    "url": "https://example.com/dest",
    "created": 1699990000,
    "country": "BR",
    "device": "Mobile",
    "referrer": "https://twitter.com/",
    "ts": 1699999999
  }
}
```

`country` e `referrer` são omitidos quando a requisição não os carregava.
`device` está sempre presente (cai para `"Other"` quando o user agent não
pode ser classificado).

`link.threshold_reached` carrega a janela que disparou o alerta, além de
`code`:

```json
{
  "id": "evt_...",
  "type": "link.threshold_reached",
  "timestamp": 1699999999,
  "data": {
    "code": "aB3xZ9k",
    "count": 100,
    "threshold": 100,
    "window_secs": 300,
    "ts": 1699999999
  }
}
```

- `count`: a contagem de cliques que cruzou o limiar nessa janela.
- `threshold` / `window_secs`: a regra configurada.
- `ts`: unix seconds do clique que disparou o alerta.

## Headers

Toda requisição carrega três headers, seguindo o esquema simétrico do
[Standard Webhooks](https://www.standardwebhooks.com/):

| Header | Significado |
|---|---|
| `webhook-id` | Estável por entrega, reusado em toda tentativa de retry dessa entrega. Use como chave de idempotência: se você já processou esse id, ignore a requisição. |
| `webhook-timestamp` | Unix seconds de quando a requisição foi assinada. Rejeite requisições com mais de 5 minutos de idade (ou visivelmente no futuro): essa é a janela de replay. |
| `webhook-signature` | `v1,<base64>`. Uma lista separada por espaço de entradas `v1,...`; basta uma delas bater na verificação. Várias entradas só aparecem durante rotação de segredo. |

## Verificando a assinatura

A string assinada é `{webhook-id}.{webhook-timestamp}.{body}` (pontos
literais, os bytes exatos do corpo da requisição, não uma cópia
re-serializada dele). O segredo é exibido e armazenado como
`whsec_<base64>`; a chave que você usa no HMAC são os bytes crus depois de
decodificar em base64 tudo que vem depois do prefixo `whsec_`.

`signature = "v1," + base64(HMAC-SHA256(chave, string_assinada))`

### Node.js

```js
const crypto = require('crypto');

function verifyWebhook(secret, webhookId, timestamp, body, signatureHeader) {
  const now = Math.floor(Date.now() / 1000);
  if (Math.abs(now - Number(timestamp)) > 300) {
    throw new Error('timestamp outside the 5-minute tolerance');
  }

  const signedString = `${webhookId}.${timestamp}.${body}`;
  const key = Buffer.from(secret.replace(/^whsec_/, ''), 'base64');
  const expected = crypto.createHmac('sha256', key).update(signedString).digest();

  const candidates = signatureHeader.split(' ').map((entry) => entry.split(',')[1]);
  return candidates.some((candidate) => {
    const candidateBuf = Buffer.from(candidate, 'base64');
    return candidateBuf.length === expected.length && crypto.timingSafeEqual(candidateBuf, expected);
  });
}

// Uso num handler Express:
// const ok = verifyWebhook(secret, req.header('webhook-id'), req.header('webhook-timestamp'),
//   rawBody, req.header('webhook-signature'));
```

`rawBody` precisa ser a string (ou buffer) do corpo da requisição intocada,
capturada antes de qualquer middleware de parsing JSON reescrevê-la.

### Python

```python
import base64
import hashlib
import hmac
import time


def verify_webhook(secret: str, webhook_id: str, timestamp: str, body: str, signature_header: str) -> bool:
    now = int(time.time())
    if abs(now - int(timestamp)) > 300:
        raise ValueError("timestamp outside the 5-minute tolerance")

    signed_string = f"{webhook_id}.{timestamp}.{body}"
    key = base64.b64decode(secret.removeprefix("whsec_"))
    expected = hmac.new(key, signed_string.encode(), hashlib.sha256).digest()

    for entry in signature_header.split(" "):
        _, candidate = entry.split(",", 1)
        if hmac.compare_digest(base64.b64decode(candidate), expected):
            return True
    return False
```

`body` precisa ser a string exata do corpo da requisição que seu framework
te entrega antes de qualquer desserialização JSON, já que uma cópia
re-serializada pode diferir byte a byte (ordem das chaves, espaços em branco)
e falharia na comparação mesmo para um evento genuíno.

### Vetor de teste

Este é o vetor de teste padrão usado para checar os dois trechos acima
contra o próprio assinador do quark:

```
secret:    whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw
webhook-id:        msg_p5jXN8AQM9LWM0D4loKWxJek
webhook-timestamp: 1614265330
body:      {"test": 2432232314}
signature: v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE=
```

Note o espaço literal depois dos dois-pontos no corpo; um
`{"test":2432232314}` compacto produz uma assinatura diferente.

## Replay e idempotência

- Rejeite qualquer entrega cujo `webhook-timestamp` tenha mais de 5 minutos.
  Uma requisição capturada e reenviada fora dessa janela deve ser descartada.
- Use `webhook-id` como chave de idempotência. A própria lógica de retry do
  quark pode reentregar o mesmo evento (timeout de rede, resposta 5xx, e por
  aí vai), então o seu receptor deve tratar um id repetido como no-op, não
  como um novo efeito colateral.

## Entrega durável (Postgres)

No backend Postgres os eventos de ciclo de vida (`link.created`,
`link.updated`, `link.deleted`) não andam pela fila em memória. Cada um é
escrito numa outbox durável e entregue por um worker de relay, então um
receptor fora do ar, ou um restart do quark, deixa de te custar o evento.

**A outbox.** Quando um evento de ciclo de vida dispara, o quark escreve uma
linha por assinatura ativa que casa na tabela `webhook_deliveries` (corpo do
evento, assinatura de destino, contagem de tentativas, horário da próxima
tentativa, uma flag de morto). A escrita comita antes da requisição admin
retornar. Se uma assinatura fica uma hora fora do ar, as linhas dela ficam na
outbox e são retentadas o tempo todo; nada se perde numa fila cheia.

**O relay com lease.** Um worker em background consulta a outbox num intervalo
curto e reivindica um lote de linhas devidas com `SELECT ... FOR UPDATE SKIP
LOCKED`. Daí saem duas coisas:

- Rode o quark em vários nós e cada relay reivindica um conjunto disjunto de
  linhas, então a mesma entrega nunca é enviada duas vezes por dois nós ao
  mesmo tempo.
- Um endpoint lento ou quebrado só trava as próprias linhas. As outras
  assinaturas são reivindicadas e entregues em paralelo, sem bloqueio de
  cabeça de fila.

Para cada linha reivindicada o relay busca a assinatura, aplica a mesma guarda
de SSRF de qualquer outra entrega, assina o corpo (Generic) ou o formata para
o canal (Slack/Discord/Telegram), e faz o POST. Num 2xx a linha é marcada como
entregue. Numa falha a contagem de tentativas sobe e o horário da próxima
tentativa é empurrado com backoff exponencial mais jitter, tudo persistido,
então o cronograma sobrevive a um restart. Esgotado o orçamento de tentativas,
a linha é marcada `dead`: vai pra fila de mortos e para de ser reivindicada.

**Pelo menos uma vez, e idempotência.** Isto é entrega pelo menos uma vez: um
crash entre um POST bem-sucedido e a linha ser marcada como entregue vai
reentregar na próxima consulta. Deduplique pelo header `webhook-id`. Numa
entrega pela outbox esse header é a chave de entrega estável da linha,
`"<event_id>.<subscription_id>"`, idêntica em toda tentativa e em todo nó.
Trate um `webhook-id` repetido como no-op (é a mesma regra de [Replay e
idempotência](#replay-e-idempotência); o caminho durável só torna o id estável
e persistido em vez de aleatório por tentativa).

**A inserção é atômica com a mutação.** As linhas da outbox entram na mesma
transação da mutação do link que gerou o evento: os fluxos de criar, editar e
apagar montam as linhas de entrega que casam (uma leitura das assinaturas
ativas, fora da transação) e as passam para a camada de armazenamento, que
grava a mudança do link junto com os inserts `ON CONFLICT (delivery_key) DO
NOTHING`. Ou os dois comitam ou nenhum, então um crash não perde mais um evento
entre a gravação do link e a inserção na outbox.

## Configurando uma assinatura

### Painel

Abra a aba **Webhooks** no painel admin. Adicione uma URL de destino, escolha
quais eventos ela deve receber, e salve. O segredo de assinatura é
mostrado uma única vez, no momento da criação; copie antes de sair dessa
tela, já que o quark nunca mostra ele de novo (só um `whsec_••••` mascarado
depois disso).

### API

Todos os endpoints de webhook vivem sob `QUARK_ADMIN_TOKEN` (header
`x-admin-token`).

```bash
# criar uma assinatura
curl -X POST localhost:8080/admin/webhooks \
  -H 'x-admin-token: <token>' -H 'content-type: application/json' \
  -d '{"url": "https://hooks.example.com/quark", "events": ["link.created", "link.clicked"]}'
# => {"id": 1, "secret": "whsec_..."}   (o segredo é retornado só essa vez)

# listar assinaturas (segredo mascarado)
curl localhost:8080/admin/webhooks -H 'x-admin-token: <token>'

# atualizar eventos ou a flag de ativo
curl -X PATCH localhost:8080/admin/webhooks/1 \
  -H 'x-admin-token: <token>' -H 'content-type: application/json' \
  -d '{"active": false}'

# remover uma assinatura
curl -X DELETE localhost:8080/admin/webhooks/1 -H 'x-admin-token: <token>'

# enviar um evento de teste sintético pra checar seu receptor
curl -X POST localhost:8080/admin/webhooks/1/test -H 'x-admin-token: <token>'
```

`url` precisa ser `http` ou `https` e não pode resolver pra um host interno
ou loopback: a mesma guarda contra SSRF (`is_internal_host`) que protege os
destinos de link se aplica aqui, checada tanto na criação da assinatura
quanto de novo no momento da entrega. Um deployment tem um teto de 50
assinaturas.

### Distinguindo Zapier, Make e n8n

Zapier, Make e n8n registram uma assinatura `kind: "generic"` simples: o
quark não tem nenhuma integração especial com nenhum deles, eles só recebem
um webhook. Historicamente isso significava que o painel de integrações
também não conseguia diferenciá-los, já que a única coisa que ele tinha para
comparar era o `kind`, e qualquer um dos três conectado acendia os três
juntos.

O `CreateReq` agora aceita um `connector_id` opcional (um id do catálogo,
ex. `"zapier"`, `"make"`, `"n8n"`). A página de cada integração no painel
envia o próprio id ao criar a assinatura, então o painel consegue casar uma
linha de volta com o conector exato que a criou, em vez de adivinhar pelo
`kind`. Uma assinatura criada antes disso existir não tem `connector_id`; o
painel cai de volta no casamento antigo por `kind` nesses casos, então nada
que já estava conectado quebra.

```bash
curl -X POST localhost:8080/admin/webhooks \
  -H 'x-admin-token: <token>' -H 'content-type: application/json' \
  -d '{"url": "https://hooks.zapier.com/hooks/catch/000000/xxxxxx", "events": ["link.created"], "connector_id": "zapier"}'
```

## Health da conexão

Cada linha de assinatura no painel e na API mostra o resultado da última
entrega: `last_delivery_status` é um de `never` (criada, nada entregue
ainda), `ok` (a última tentativa deu certo) ou `error` (com um `detail`
curto, nunca um segredo ou token). `last_delivery_at` é o timestamp unix
dessa tentativa. Pixels carregam o mesmo formato em `last_forward_status` /
`last_forward_at` para o último forward de conversão.

Isso é **health passivo**: o quark nunca faz polling num receptor pra
perguntar "você ainda está aí?". O status é só o que a última tentativa real
de entrega retornou, gravado como efeito colateral de enviá-la.

`link.clicked` é excluído de propósito. É o único evento que dispara do
caminho quente do redirect, e gravar health ali significaria uma escrita por
clique, exatamente o custo que o design assíncrono e best-effort desse
evento existe para evitar (veja [Eventos](#eventos)). Então o health
reflete:

- os eventos de ciclo de vida: `link.created`, `link.updated`,
  `link.deleted`, `link.expired`, `link.threshold_reached`,
- e o botão `/admin/webhooks/:id/test`, que sempre grava health já que
  nunca é um clique de verdade.

Uma assinatura que só recebe `link.clicked` pode ficar em `never`
indefinidamente mesmo entregando cliques direitinho. Isso é esperado: use o
"Testar" pra checar, ou assine também um evento de ciclo de vida se você
quer que o status reflita o tráfego ao vivo.

O health de pixel segue o caminho do forward de conversão em [Conversion
forwarding](CONVERSION-FORWARDING.PT_BR.md), que já roda fora do caminho
quente do redirect no worker de analytics; a gravação ali é best-effort e
nunca trava o próximo lote desse worker.

## Alertas de limiar de cliques

Um link pode carregar uma regra de alerta: disparar `link.threshold_reached`
quando ele for clicado pelo menos `threshold` vezes dentro de uma janela fixa
de `window_secs` segundos. É útil pra perceber um link que viraliza de repente,
ou pra pegar fraude de cliques cedo.

A contagem usa um contador de janela fixa, a mesma abordagem do rate limiter:
a janela é `floor(ts_do_clique / window_secs)`, e o evento dispara uma vez por
janela, no clique que primeiro leva a contagem da janela a `threshold`. Cliques
seguintes na mesma janela não re-disparam; a janela seguinte começa uma
contagem nova e pode disparar de novo. Quando o quark roda com Valkey o
contador é compartilhado entre todas as réplicas (exato no cluster inteiro);
sem Valkey cada réplica conta por conta própria (exato num nó único). A
contagem e a entrega rodam no worker de analytics, fora do caminho quente do
redirect, e são fail-open: um erro de Valkey é logado e nunca bloqueia um
redirect.

Pra receber o evento, uma assinatura de webhook (ou de canal) precisa incluir
`link.threshold_reached` nos seus `events`, exatamente como qualquer outro
evento.

### Configurando a regra (API)

A regra é definida por link, chaveada pelo short code do link, sob
`QUARK_ADMIN_TOKEN` (header `x-admin-token`). `threshold` precisa ser `>= 1` e
`window_secs` precisa ser `>= 60`.

```bash
# definir (ou substituir) a regra de alerta de um link:
# 100 cliques em 5 minutos
curl -X PUT localhost:8080/admin/links/aB3xZ9k/alert \
  -H 'x-admin-token: <token>' -H 'content-type: application/json' \
  -d '{"threshold": 100, "window_secs": 300}'
# => {"threshold": 100, "window_secs": 300}

# remover a regra de alerta
curl -X DELETE localhost:8080/admin/links/aB3xZ9k/alert -H 'x-admin-token: <token>'
# => 204 No Content
```

`:code` aceita tanto o short code canônico quanto um alias customizado, igual
aos outros endpoints `/admin/links/:code`.

### Template n8n

Monte um fluxo que reage a `link.threshold_reached`:

1. Nó **Webhook** (gatilho): método `POST`, copie a URL de Test/Production dele,
   e registre como uma assinatura que inclui o evento:

   ```bash
   curl -X POST localhost:8080/admin/webhooks \
     -H 'x-admin-token: <token>' -H 'content-type: application/json' \
     -d '{"url": "https://<seu-host-n8n>/webhook/quark", "events": ["link.threshold_reached"]}'
   ```

2. Nó **IF** (filtro opcional): continue só para um link específico, ex.
   expressão `{{ $json.body.data.code }}` igual a `aB3xZ9k`.
3. Nó de **ação**: Slack "Send Message", Email, ou um HTTP Request. Referencie
   os campos do payload direto, por exemplo:

   ```
   Link {{ $json.body.data.code }} atingiu {{ $json.body.data.count }} cliques
   em {{ $json.body.data.window_secs }}s.
   ```

Pra verificar a assinatura dentro do n8n, adicione um nó **Code** antes da ação
e porte o [trecho Node.js](#nodejs) acima, lendo `webhook-id`,
`webhook-timestamp` e `webhook-signature` de `$json.headers`.

### Template Zapier

1. **Gatilho**: "Webhooks by Zapier" -> "Catch Hook". O Zapier te dá uma URL
   customizada; registre ela como uma assinatura com o evento:

   ```bash
   curl -X POST localhost:8080/admin/webhooks \
     -H 'x-admin-token: <token>' -H 'content-type: application/json' \
     -d '{"url": "https://hooks.zapier.com/hooks/catch/000000/xxxxxx", "events": ["link.threshold_reached"]}'
   ```

2. **Filtro** (opcional): "Only continue if..." -> `Data Code` -> `(Text)
   Exactly matches` -> `aB3xZ9k`.
3. **Ação**: qualquer app do Zapier, ex. Slack "Send Channel Message" ou Gmail
   "Send Email". Mapeie os campos capturados `data__code`, `data__count` e
   `data__window_secs` no corpo da mensagem.

O Catch Hook do Zapier não verifica a assinatura HMAC. Se você precisa de
autenticidade, use "Catch Raw Hook" e adicione um passo Code que reimplementa a
[verificação](#verificando-a-assinatura), ou mantenha a URL do webhook secreta
e confie nela ser não-adivinhável.

## Canais de notificação

Uma assinatura tem um `kind`: `generic` (default, descrito acima) ou um dos
três canais de chat, `slack`, `discord`, `telegram`. Escolha um canal no
seletor "Type" do diálogo de criação (ou passe `kind` na API) quando tudo que
você quer é uma mensagem simples num chat, não uma integração assinada.

A diferença central: **canais não são assinados**. Não há HMAC, não há
headers `webhook-*`, e não há `secret`. A autenticação é a própria URL: a URL
de entrada de cada canal é uma credencial portadora, então quem tiver essa
URL consegue postar no seu canal. Trate ela com o mesmo cuidado que uma
senha. A guarda contra SSRF e a política de não seguir redirect continuam
valendo pras URLs de canal, igual ao Generic.

### Como conseguir a URL de cada canal

**Slack.** Adicione o app "Incoming Webhooks" no seu workspace (ou abra a
configuração de um app já existente), ative incoming webhooks e crie um pro
canal que você quer. O Slack te dá uma URL no formato
`https://hooks.slack.com/services/T000/B000/XXXXXXXX`. Cole isso como a URL
da assinatura.

**Discord.** No canal alvo, abra Server Settings > Integrations > Webhooks,
crie um webhook novo e copie a URL dele
(`https://discord.com/api/webhooks/<id>/<token>`). Cole isso como a URL da
assinatura.

**Telegram.** Mande mensagem pro [@BotFather](https://t.me/BotFather) pra
criar um bot e pegar o token dele. Encontre o `chat_id` numérico do chat onde
você quer as mensagens (um chat privado, grupo, ou canal do qual seu bot é
membro). Monte a URL você mesmo:

```
https://api.telegram.org/bot<TOKEN>/sendMessage?chat_id=<ID>
```

Cole essa URL inteira como a URL da assinatura. O quark faz POST de `text` no
corpo JSON; o Telegram lê `chat_id` da query string e `text` do corpo.

### Formato da mensagem

O quark deriva uma mensagem curta em texto plano a partir dos mesmos dados de
evento que as assinaturas Generic recebem, e embrulha ela no formato que cada
canal espera:

| Evento | Mensagem |
|---|---|
| `link.created` | `New short link: {code} -> {url}` |
| `link.updated` | `Short link updated: {code} -> {url}` |
| `link.deleted` | `Short link deleted: {code}` |
| `link.expired` | `Short link expired: {code}` |
| `link.clicked` | `Click on {code} -> {url}`, com ` ({country})` acrescentado quando o clique carregava um país |
| `link.threshold_reached` | `Click threshold reached for {code}: {count} clicks in {window_secs}s` |

Slack e Telegram recebem os dois:

```json
{"text": "New short link: aB3xZ9k -> https://example.com/dest"}
```

Discord recebe:

```json
{"content": "New short link: aB3xZ9k -> https://example.com/dest"}
```

É texto plano, sem marcação de formatação. O Block Kit do Slack e os embeds
ricos do Discord são formatos de mensagem mais elaborados que os dois canais
suportam; o quark não constrói eles hoje. Isso é uma melhoria de formatação
futura, não algo que você já pode ativar.

### API

Assinaturas de canal usam os mesmos endpoints do Generic, com `kind` na
requisição de criação:

```bash
curl -X POST localhost:8080/admin/webhooks \
  -H 'x-admin-token: <token>' -H 'content-type: application/json' \
  -d '{"url": "https://hooks.slack.com/services/T000/B000/XXXXXXXX", "events": ["link.created"], "kind": "slack"}'
# => {"id": 2}   (sem o campo "secret": canais não são assinados)
```

`kind` é um de `"generic"`, `"slack"`, `"discord"`, `"telegram"`; o default é
`"generic"` quando omitido.
