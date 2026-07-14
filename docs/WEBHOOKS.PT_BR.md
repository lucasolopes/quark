[English](WEBHOOKS.md) · **Português**

# Webhooks

Uma instância quark de operador único pode enviar eventos HTTP assinados para
um endpoint externo (Zapier, Make, n8n, Slack ou qualquer receptor
customizado). A entrega é best-effort: os eventos passam por uma fila
limitada em memória, um worker assina e envia via POST com retry (backoff
exponencial mais jitter), e uma assinatura que fica fora do ar além do
orçamento de retry simplesmente perde aquele evento. A *configuração* da
assinatura (URL, conjunto de eventos, segredo, flag de ativo) é durável,
persistida no store (LMDB ou Postgres); a entrega em si não é.

Se você precisa de entrega durável, que sobrevive a um restart, com log de
tentativas persistido, isso é uma melhoria futura condicionada ao Postgres,
não o que este documento cobre hoje.

## Eventos

| Evento | Dispara quando |
|---|---|
| `link.created` | Um link é criado (`POST /`). |
| `link.updated` | Um link é editado (`PATCH /admin/links/:code`). |
| `link.deleted` | Um link é removido (`DELETE /admin/links/:code`). |
| `link.expired` | Um redirect resolve um link além do TTL (o caminho do `410 Gone`). Não há sweeper em background: a expiração é observada no acesso, igual ao resto do tratamento de TTL do quark. |
| `link.clicked` | Um redirect tem sucesso. Emitido pelo caminho assíncrono, nunca pelo caminho quente do 302, e só quando pelo menos uma assinatura ativa quer esse evento (uma flag atômica em cache mantém o custo zero no resto do tempo). |

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
- `type`: um dos cinco nomes de evento acima.
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

## Configurando uma assinatura

### Painel

Abra a aba **Webhooks** no painel admin. Adicione uma URL de destino, escolha
quais dos cinco eventos ela deve receber, e salve. O segredo de assinatura é
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
