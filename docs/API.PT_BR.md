[English](API.md) · **Português**

# Referência da API HTTP

Toda rota que o quark serve, de `src/api.rs` (`router_with_cors`). Duas rotas
são públicas por default (`POST /` e `GET /:code`); os arquivos well-known são
sempre públicos; tudo em `/admin/*` e `GET /:code/stats` é protegido.

Timestamps (`created`, `expiry`, `ts`, `timestamp`) são em segundos unix. Um
`code` é uma string base62 computada de 7 caracteres ou um alias customizado.

## Autenticação

Requisições de admin levam o token no header `x-admin-token`. Dois tipos de
token são aceitos:

- O env `QUARK_ADMIN_TOKEN`, comparado em tempo constante. Sempre tem o escopo
  `full`.
- Um token de API nomeado (`qtok_...`), buscado pelo hash SHA-256. Só é liberado
  se os escopos cobrirem o que o endpoint exige, e pode carregar um rate limit
  por token. Veja [API-TOKENS](API-TOKENS.PT_BR.md).

O escopo que cada endpoint de admin exige está listado por rota abaixo. Status
comuns quando a checagem de escopo falha:

| Status | Quando |
|---|---|
| `401 Unauthorized` | Token ausente ou desconhecido, e `QUARK_ADMIN_TOKEN` está configurado. |
| `403 Forbidden` | Token de API válido cujos escopos não cobrem o exigido. |
| `404 Not Found` | Token ausente ou desconhecido, e sem `QUARK_ADMIN_TOKEN` (o admin fica desligado). |
| `429 Too Many Requests` | Token de API válido acima do próprio `rate_limit_per_min`. |
| `503 Service Unavailable` | O store falhou ao checar o token. |

## Rotas públicas

### `GET /health`

Checagem de liveness. Sem auth. Retorna `200` com o corpo `ok`.

### `POST /`

Cria um link curto. Público quando `QUARK_ADMIN_TOKEN` está sem setar; senão
exige o env token ou um token de API que cubra `links_write`.

Corpo da requisição (`application/json`):

| Campo | Tipo | Notas |
|---|---|---|
| `url` | string, obrigatório | Deve começar com `http://` ou `https://`. |
| `alias` | string, opcional | Código customizado. Não pode ser um base62 válido de 7 chars na faixa. |
| `ttl` | número, opcional | Segundos até expirar, a partir de agora. |
| `tags` | array de string, opcional | Normalizadas: trim, minúsculas, dedup, teto de 20. |
| `max_visits` | número, opcional | Expira após tantas visitas. `0` ou ausente é ilimitado. |
| `rules` | array, opcional | Regras de redirect geo/dispositivo, até 20. Veja [REDIRECT-RULES](REDIRECT-RULES.PT_BR.md). |
| `variants` | array, opcional | Variantes A/B com peso, até 10. Veja [AB-TESTING](AB-TESTING.PT_BR.md). |
| `app_ios` | string, opcional | Destino deep-link iOS. Veja [DEEP-LINKING](DEEP-LINKING.PT_BR.md). |
| `app_android` | string, opcional | Destino deep-link Android. |
| `folder` | string, opcional | Uma pasta a que o link pertence. Trim, teto de 48 chars, case preservado; vazio vira nenhuma. |
| `fallback_url` | string, opcional | Para onde mandar o visitante quando o link já expirou (por TTL ou `max_visits`) em vez de `410`. `http`/`https`, não-interna; vazio vira nenhuma. |
| `password` | string, opcional | Protege o link com senha. Guardada como hash argon2id; o texto puro nunca é persistido nem devolvido. Vazia/ausente = sem senha. |

Sucesso: `200` com `{"code": "...", "url": "..."}`.

Falhas (cada uma com corpo em texto):

| Status | Motivo |
|---|---|
| `400 Bad Request` | url inválida, url sem host, ttl inválido, alias colide com o espaço numérico, regras/variantes demais, valor de device inválido, peso de variante < 1 |
| `403 Forbidden` | destino bloqueado (interno/self), na url, no `to` de uma regra ou na url de variante |
| `409 Conflict` | alias em uso |
| `429 Too Many Requests` | acima de `QUARK_RATELIMIT_PER_MIN` para o IP |
| `507 Insufficient Storage` | espaço de id esgotado |
| `503 Service Unavailable` | erro de backend |

```bash
curl -X POST localhost:8080/ -H 'content-type: application/json' \
  -d '{"url": "https://example.com/some/long/path", "ttl": 3600}'
# => {"code":"01aB2Cd","url":"https://example.com/some/long/path"}
```

### `GET /:code`

Resolve e redireciona. Sem auth. O quark decodifica um código base62 numérico
por aritmética primeiro, depois cai num lookup de alias.

Respostas:

| Status | Quando | Headers |
|---|---|---|
| `302 Found` | Link resolvido e vivo. | `Location`, `Cache-Control` ciente do TTL. |
| `302 Found` | Expirado (TTL ou `max_visits`) e há um `fallback_url`. | `Location: <fallback_url>`, `Cache-Control: no-store`. |
| `410 Gone` | Expirado (TTL ou `max_visits`) sem `fallback_url`. | `Cache-Control: no-store`. |
| `200 OK` | Link protegido por senha e a requisição não tem cookie de unlock válido. | interstitial `text/html`, `Cache-Control: no-store`. |
| `404 Not Found` | Sem tal código ou alias. | `Cache-Control: no-store`. |
| `503 Service Unavailable` | Erro de backend. | |

A resolução do destino compõe três mecanismos de segmentação em ordem de
prioridade: um deep-link de app por dispositivo ganha primeiro, depois uma regra
geo/dispositivo que casa, depois uma variante A/B por peso, e um link sem nada
disso redireciona para o `url`. Veja
[ARCHITECTURE](ARCHITECTURE.PT_BR.md#fluxo-de-redirect).

### `POST /:code`

Desbloqueia um link protegido por senha. Público, rate-limited (por IP do
cliente, compartilhado com o create). Corpo `application/x-www-form-urlencoded`
com um campo `password`.

| Status | Quando | Headers |
|---|---|---|
| `303 See Other` | Senha correta. | `Location: /<code>`, `Set-Cookie: qk_pw_<code>=…` (assinado, `HttpOnly`, `SameSite=Lax`, 12h), `Cache-Control: no-store`. |
| `200 OK` | Senha errada. | interstitial `text/html` com erro, sem cookie. |
| `429 Too Many Requests` | Acima do limite. | |

No sucesso o quark redireciona de volta pro `GET /:code`; a requisição seguinte
leva o cookie de unlock, então a resolução de destino, o incremento de visitas e
o registro do clique acontecem uma vez só no caminho canônico. O cookie deixa
visitas repetidas em 12h pularem o interstitial. A senha é verificada contra um
hash argon2id; o texto puro nunca é armazenado.

### Arquivos well-known (deep linking)

Públicos, servidos como `application/json` sem redirect. `200` com o corpo
armazenado, ou `404` quando não configurado. Veja
[DEEP-LINKING](DEEP-LINKING.PT_BR.md).

| Rota | Arquivo |
|---|---|
| `GET /.well-known/apple-app-site-association` | AASA (iOS) |
| `GET /apple-app-site-association` | AASA (iOS, caminho raiz legado) |
| `GET /.well-known/assetlinks.json` | Digital Asset Links (Android) |

## Analytics

### `GET /:code/stats`

Analytics de clique por link. Escopo: `analytics`. `404` se o código não
resolver para um link armazenado.

Sucesso: `200` com `{"aggregates": {...}, "recent": [...]}`. `aggregates` tem
`total`, `bots`, `first_ts`, `last_ts` e os mapas `per_day`, `per_country`,
`per_device`, `per_os`, `per_browser`, `per_referer`, `per_city`,
`per_variant`. `recent` são os eventos de clique mais novos (até 1000). Um link
sem cliques retorna agregados vazios e lista vazia. Veja
[ANALYTICS](ANALYTICS.PT_BR.md).

## Gestão de links

### `GET /admin/links`

Lista links, paginado por keyset. Escopo: `links_read`.

Parâmetros de query: `after` (cursor de id), `limit` (default 50, teto 500),
`q` (busca em url e alias, só Postgres), `tag` (filtra por uma tag),
`folder` (filtra por nome de pasta, sem diferenciar maiúsculas).

Sucesso: `200` com `{"links": [...], "next_after": <id ou null>}`. Cada linha
traz `id`, `code`, `alias` opcional, `url`, `expiry`, `created`, `tags`,
`max_visits` opcional, `visits`, `rules`, `variants`, um `folder` opcional
(omitido quando o link não tem pasta), um `fallback_url` opcional, e
`has_password` (um bool; o hash da senha nunca é devolvido).

`501 Not Implemented` volta quando `q` é usado no backend LMDB (busca é só
Postgres; o painel cai para filtro client-side).

### `GET /admin/tags`

As tags distintas entre todos os links com a contagem de links, para o filtro do
painel. Escopo: `links_read`. Retorna
`{"tags": [{"name": "...", "count": N}, ...]}`, ordenado por nome. Uma tag
repetida no mesmo link conta esse link uma vez.

### `GET /admin/folders`

Os nomes de pasta distintos com a contagem de links, para o seletor e o filtro
de pasta do painel. Escopo: `links_read`. Retorna
`{"folders": [{"name": "...", "count": N}, ...]}`, ordenado por nome. Links sem
pasta não entram na contagem.

### `PATCH /admin/links/:code`

Edita um link. Escopo: `links_write`. O corpo é um objeto JSON parcial; só as
chaves presentes mudam. Mandar `null` (ou, para `fallback_url`/`password`,
string vazia) para `ttl`, `max_visits`, `app_ios`, `app_android`, `folder`,
`fallback_url` ou `password` limpa o campo. Um `password` não-vazio define um
novo hash.

Chaves aceitas: `url`, `ttl`, `tags`, `max_visits`, `rules`, `variants`,
`app_ios`, `app_android`, `folder`, `fallback_url`, `password`. Cada uma é validada como na criação (esquema
de URL, guard SSRF, tetos de regra e variante; o nome da pasta é aparado e
limitado). `200` no sucesso, `404` se o código não resolve, `400`/`403` num
campo rejeitado.

### `DELETE /admin/links/:code`

Remove um link (e o alias, se o código era alias). Escopo: `links_write`. `200`
no sucesso, `404` se não resolve.

### `POST /admin/import`

Cria links em lote a partir de um corpo CSV ou JSON. Escopo: `links_write`.
Sempre com gate de admin, mesmo com `POST /` público ligado. Nunca aborta numa
linha ruim; retorna `200` com `{"imported": N, "failed": [{index, url, reason}, ...]}`.
Um corpo acima de 10.000 linhas ou não parseável é `400`. Veja
[IMPORT](IMPORT.PT_BR.md).

## Webhooks

Gestão de assinaturas. Escopo: `webhooks` em toda rota. Veja
[WEBHOOKS](WEBHOOKS.PT_BR.md) para eventos, payload e assinatura.

| Rota | Função |
|---|---|
| `GET /admin/webhooks` | Lista assinaturas (secret mascarado). `200 {"webhooks": [...]}`. |
| `POST /admin/webhooks` | Cria. Corpo `{url, events, active?, kind?}`. `201 {"id", "secret"?}`. O secret de assinatura volta uma vez, só para `generic`. |
| `PATCH /admin/webhooks/:id` | Atualiza `url`, `events`, `active` ou `kind`. `200`, ou `404`. |
| `DELETE /admin/webhooks/:id` | Remove. `204`, ou `404`. |
| `POST /admin/webhooks/:id/test` | Entrega um `link.created` sintético uma vez, síncrono. Retorna `{"delivered": bool, "status"?: N, "error"?: "..."}`. |

`url` deve ser `http`/`https` e passar no guard SSRF. Um deploy limita em 50
assinaturas (`400` além disso). `kind` é `generic` (default, assinado), `slack`,
`discord` ou `telegram` (mensagens de canal sem assinatura).

## Pixels de conversão

Configs GA4 / Meta CAPI no nível da instância. Escopo: `analytics`. Veja
[CONVERSION-FORWARDING](CONVERSION-FORWARDING.PT_BR.md).

| Rota | Função |
|---|---|
| `GET /admin/pixels` | Lista configs, secrets mascarados. `200 {"pixels": [...]}`. |
| `POST /admin/pixels` | Cria. Corpo `{provider, credentials, active?}` onde `provider` é `ga4` ou `meta_capi`. `201` com a linha mascarada, ou `400` por credencial faltando ou o teto de 20 configs. |
| `DELETE /admin/pixels/:id` | Remove. `204`, ou `404`. |

## Tokens de API

Gestão de tokens. Escopo: `full` em toda rota (só um token superusuário gere
tokens). Veja [API-TOKENS](API-TOKENS.PT_BR.md).

| Rota | Função |
|---|---|
| `GET /admin/tokens` | Lista tokens (nunca o hash ou o texto puro). `200 {"tokens": [...]}`. |
| `POST /admin/tokens` | Cria. Corpo `{name, scopes, rate_limit_per_min?}`. `201 {"id", "token"}` com o texto puro uma vez. `400` no teto de 100 tokens. |
| `DELETE /admin/tokens/:id` | Revoga. `204`, ou `404`. |

## Documentos well-known (admin)

Gerencia os arquivos de associação de deep linking. Escopo: `full`. `:name`
deve ser `apple-app-site-association` ou `assetlinks.json`, senão `404`.

| Rota | Função |
|---|---|
| `GET /admin/wellknown/:name` | Lê o corpo armazenado. `200` com o corpo, ou `200` com corpo vazio quando não configurado (o painel trata vazio como "não configurado"). |
| `PUT /admin/wellknown/:name` | Grava um corpo. Deve ser JSON válido em até 64 KiB, senão `400`. `200` no sucesso. |
| `DELETE /admin/wellknown/:name` | Remove; o caminho público volta a `404`. `204`. |

## CORS

Com `QUARK_CORS_ORIGINS` setado (vírgula), o quark adiciona uma camada de CORS
liberando essas origens com `GET, POST, PUT, PATCH, DELETE` e qualquer header.
Sem setar é só mesma origem. É para o painel web hospedado à parte.
