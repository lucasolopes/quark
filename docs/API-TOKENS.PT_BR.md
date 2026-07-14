[English](API-TOKENS.md) · **Português**

# Tokens de API

Além do único `QUARK_ADMIN_TOKEN`, o quark suporta tokens de API nomeados
com permissões por escopo e um limite de requisições opcional por token.
Isso permite que um script, um pipeline de CI ou uma integração tenha um
token mais restrito que o acesso total de admin, e permite revogar aquela
integração específica sem precisar rotacionar o token do próprio operador.

## Escopos

Cada token recebe um ou mais escopos. Uma requisição só é permitida se os
escopos do token cobrirem o que o endpoint exige.

| Escopo | Permite |
|---|---|
| `links_read` | Listar links (`GET /admin/links`), incluindo busca. |
| `links_write` | Criar, editar e excluir links (`POST /`, `PATCH`/`DELETE /admin/links/:code`, importação, escrita de tags). |
| `blocklist` | Gerenciar a blocklist de destinos (`GET`/`POST`/`DELETE /admin/blocklist`). |
| `webhooks` | Gerenciar a configuração de webhooks. |
| `analytics` | Ler estatísticas de cliques (`GET /:code/stats`). |
| `full` | Superusuário: cobre todos os escopos acima, incluindo gerenciamento de tokens (`/admin/tokens`). Só tokens `full` podem criar, listar ou revogar outros tokens. |

O env `QUARK_ADMIN_TOKEN` sempre se comporta como `full`, sem mudanças em
relação a antes de tokens de API existirem.

## Usando um token

Envie o token no header `x-admin-token`, exatamente como o token de admin
do env:

```bash
# cria um token (exige um token full/superusuário, ex.: QUARK_ADMIN_TOKEN)
curl -X POST https://seu-host-quark/admin/tokens \
  -H 'x-admin-token: <admin-token>' \
  -H 'content-type: application/json' \
  -d '{"name": "Pipeline de CI", "scopes": ["links_read"], "rate_limit_per_min": 60}'
# => 201 {"id": 3, "token": "qtok_...32+ caracteres..."}
# O token em texto puro só aparece NESSA resposta. Copie agora.

# usa o token em um endpoint com escopo
curl https://seu-host-quark/admin/links \
  -H 'x-admin-token: qtok_...'
```

Um token cujos escopos não cobrem o endpoint recebe `403 Forbidden`. Um
token revogado ou desconhecido recebe `401 Unauthorized` (ou `404` se
nenhum token de admin do env estiver configurado, no mesmo comportamento já
existente do token do env).

## Quota (limite de requisições por token)

Um token pode opcionalmente carregar `rate_limit_per_min`. Quando definido,
as requisições autenticadas com esse token são contadas por minuto;
ultrapassar o limite retorna `429 Too Many Requests`. Deixar sem definir
significa nenhum limite por token (o token ainda está sujeito a qualquer
limite global de requisições que o quark tenha configurado separadamente).

## O token só aparece uma vez

O token em texto puro é gerado no `POST /admin/tokens` e retornado
exatamente uma vez, nessa resposta. O quark guarda apenas o hash SHA-256
dele, nunca o texto puro. O `GET /admin/tokens` (a listagem) e qualquer
outro endpoint nunca retornam o hash ou o texto puro, só `id`, `name`,
`scopes`, `rate_limit_per_min` e `created`. Se você perder o texto puro, não
há como recuperá-lo: revogue o token e crie um novo.

## Gerenciando tokens

Os próprios endpoints de `/admin/tokens` exigem um token com escopo `full`
(ou o `QUARK_ADMIN_TOKEN` do env):

- `GET /admin/tokens`: lista os tokens (sem hash nem texto puro).
- `POST /admin/tokens` `{name, scopes, rate_limit_per_min?}`: cria um token,
  retorna `201 {id, token}` com o texto puro uma vez.
- `DELETE /admin/tokens/:id`: revoga um token, retorna `204`.

A página **Tokens de API** (`/tokens`) do painel web cobre os três: criar
com um nome, checkboxes de escopo e um limite de requisições opcional; o
token em texto puro aparece uma vez com um botão de copiar e um aviso de
que não será mostrado de novo; cada token na lista pode ser revogado com um
diálogo de confirmação.

Até 100 tokens podem existir ao mesmo tempo.
