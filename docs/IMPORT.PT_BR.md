[English](IMPORT.md) · **Português**

# Importando links (migração de outro encurtador)

`POST /admin/import` cria links em lote a partir de um arquivo CSV ou JSON.
Existe para um propósito: mover um inventário de links já existente para o
quark sem redigitar cada URL. Aponte para uma exportação do Bitly, Kutt,
YOURLS, ou qualquer planilha genérica, e ele cria um link por linha,
relatando exatamente quais linhas deram certo e quais falharam.

O endpoint é admin-only (protegido por `QUARK_ADMIN_TOKEN`), independente de
o `POST /` de criação pública estar habilitado. Cada linha passa pelas
mesmas checagens de validação, blocklist e anti-loop de um create normal via
`POST /`, então uma importação não consegue criar links que um create manual
teria rejeitado.

## Formatos

Envie JSON ou CSV. O quark escolhe o parser pelo header `Content-Type`
primeiro (`application/json`, ou `text/csv`/`application/csv`); se o header
estiver ausente ou não reconhecido, ele analisa o corpo: um `[` ou `{` no
início é tratado como JSON, qualquer outra coisa como CSV.

### JSON

Um array de objetos, um por link. `url` é obrigatório; `alias` e `ttl` são
opcionais.

```json
[
  { "url": "https://example.com/pagina/de/destino/longa", "alias": "promo", "ttl": 604800 },
  { "url": "https://example.com/outra/pagina" }
]
```

- `ttl` é em segundos, contado a partir do momento da importação (não é
  preservado do sistema de origem, que não tem o conceito de "segundos até
  expirar" numa exportação).
- Uma linha sem `alias` recebe um código calculado pelo quark, igual a um
  create normal.

### CSV

Uma linha de header, depois um link por linha. O quark detecta
automaticamente as colunas de URL, alias e TTL pelo nome (sem diferenciar
maiúsculas/minúsculas), então exportações de ferramentas diferentes
funcionam sem editar o arquivo antes.

```csv
url,alias,ttl
https://example.com/pagina/de/destino/longa,promo,604800
https://example.com/outra/pagina,,
```

## Mapeamento de colunas e campos

O quark reconhece vários nomes por campo, cobrindo o vocabulário usado por
exportações do Bitly, Kutt e YOURLS:

| Campo | Chaves JSON aceitas | Headers CSV aceitos (case-insensitive) |
|---|---|---|
| URL (obrigatório) | `url`, `long_url`, `longUrl` | `url`, `long_url`, `longurl`, `original_url`, `long` |
| Alias / código customizado (opcional) | `alias`, `keyword`, `short` | `alias`, `keyword`, `short`, `short_code`, `custom` |
| TTL em segundos (opcional) | `ttl` | `ttl`, `expiry` |

Se um CSV não tiver nenhuma coluna que bata com a lista de URL, a requisição
inteira é rejeitada com `400 Bad Request` antes de qualquer linha ser
processada (não há nada a importar). Colunas de alias ou TTL ausentes não
são um problema; esses campos ficam simplesmente vazios em toda linha.

## Migrando do Bitly

A exportação CSV do Bitly usa `long_url` para o destino e inclui colunas
que o quark ignora (contagem de cliques, data de criação, etc).

1. No dashboard do Bitly, vá até a lista de links e exporte para CSV.
2. Envie o CSV direto (ou cole o conteúdo) na página de Import do quark,
   ou mande direto via curl:

```bash
curl -X POST https://seu-host-quark/admin/import \
  -H "content-type: text/csv" \
  -H "x-admin-token: $QUARK_ADMIN_TOKEN" \
  --data-binary @bitly_export.csv
```

Os próprios códigos curtos do Bitly não são preservados (ver "O que não é
preservado" abaixo); cada link recebe um código novo do quark, a menos que
você adicione uma coluna `alias` antes de importar.

## Migrando do Kutt

O Kutt exporta em JSON ou CSV, e o quark lê os dois diretamente.

- **JSON:** os objetos de link do Kutt já usam URLs no estilo `target` e um
  campo `code` para o código curto customizado. Renomeie (ou mapeie) `code`
  para `alias`/`keyword`/`short` antes de enviar, se sua exportação ainda
  não usar um desses nomes, assim o quark trata como o alias customizado a
  manter.
- **CSV:** mesma ideia; garanta que a coluna de URL se chame `url` (ou uma
  das aliases aceitas acima) e a coluna de código curto se chame `alias`,
  `keyword`, `short`, `short_code`, ou `custom`.

```bash
curl -X POST https://seu-host-quark/admin/import \
  -H "content-type: application/json" \
  -H "x-admin-token: $QUARK_ADMIN_TOKEN" \
  --data-binary @kutt_export.json
```

## Migrando do YOURLS

A exportação CSV nativa do YOURLS já bate quase igual com o formato
esperado pelo quark: a coluna `keyword` é o código curto e a coluna `url` é
o destino, ambas reconhecidas de fábrica.

1. No admin do YOURLS, use a ferramenta "Export" (ou a API) para gerar um
   CSV com header `keyword,url,...`.
2. Importe sem editar:

```bash
curl -X POST https://seu-host-quark/admin/import \
  -H "content-type: text/csv" \
  -H "x-admin-token: $QUARK_ADMIN_TOKEN" \
  --data-binary @yourls_export.csv
```

Cada `keyword` se torna o alias que o quark tenta reaproveitar como código
curto (sujeito às regras de alias abaixo).

## O que não é preservado

A importação migra apenas o link em si (URL de destino, alias opcional, TTL
opcional). Ela não migra histórico de cliques nem analytics do sistema de
origem; esses dados ficam na ferramenta antiga.

O código curto original do sistema de origem só é mantido **se** ele chegar
como valor de `alias`/`keyword` e passar nas regras de alias do quark (não
pode se parecer com um código computado válido do quark, e não pode já
estar em uso). Se uma linha não tem alias, ou o alias é rejeitado, o quark
atribui seu próprio código calculado em vez de tentar preservar o código do
sistema de origem.

## O limite de 10.000 linhas

Uma única requisição `POST /admin/import` aceita no máximo 10.000 linhas.
É uma requisição síncrona e pesada: não há fila de job em background por
trás dela, então o limite existe pra limitar memória e tempo de execução.
Uma requisição com mais linhas que o limite é rejeitada de cara com
`400 Bad Request`, antes de qualquer linha ser importada. Divida uma
exportação maior em vários arquivos.

## Sucesso parcial e o relatório de falhas

A importação nunca aborta na primeira linha ruim. Cada linha é tentada de
forma independente, usando exatamente a mesma validação do `POST /`. A
resposta é sempre `200 OK` (uma vez que a requisição em si foi aceita) com
um resumo:

```json
{
  "imported": 2,
  "failed": [
    { "index": 3, "url": "not-a-url", "reason": "invalid url" },
    { "index": 7, "url": "https://example.com", "reason": "alias in use" }
  ]
}
```

- `imported` é a contagem de linhas que criaram um link.
- `failed` lista cada linha que não criou, com seu índice zero-based na
  requisição, sua URL, e um motivo curto: `invalid url`, `url without
  host`, `blocked destination`, `alias collides with the numeric code
  space`, `alias in use`, `invalid ttl`, `id space exhausted`, ou `backend
  error`.

Reenviar o mesmo arquivo depois de corrigir as linhas que falharam é
seguro: as linhas que já importaram mantêm seus links; só as linhas que
falharam antes precisam de atenção (normalmente editando o alias ou a URL
e reenviando só essas).

## Usando o painel web

A aba "Import" do painel aceita upload de arquivo `.csv` ou `.json`, ou um
bloco de texto colado direto (colar é útil pra um trecho JSON rápido ou um
CSV curto sem precisar salvar um arquivo antes). Depois de enviar, mostra
"Imported N, M failed" com uma tabela das linhas que falharam (índice, URL,
motivo), pra você ver exatamente o que corrigir.

## Referência curl

```bash
# corpo JSON
curl -X POST https://seu-host-quark/admin/import \
  -H "content-type: application/json" \
  -H "x-admin-token: $QUARK_ADMIN_TOKEN" \
  -d '[{"url": "https://example.com/a", "alias": "promo", "ttl": 3600}, {"url": "https://example.com/b"}]'

# corpo CSV
curl -X POST https://seu-host-quark/admin/import \
  -H "content-type: text/csv" \
  -H "x-admin-token: $QUARK_ADMIN_TOKEN" \
  --data-binary $'url,alias,ttl\nhttps://example.com/a,promo,3600\nhttps://example.com/b,,\n'
```

O `x-admin-token` é obrigatório independente de o `POST /` de criação
pública estar habilitado: `QUARK_ADMIN_TOKEN` não configurado no servidor
faz o endpoint responder `404` (igual aos outros endpoints `/admin/*`); um
token errado responde `401`.
