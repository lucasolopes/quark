[English](ANALYTICS.md) · **Português**

# Analytics de cliques e privacidade

Este documento explica o que o quark registra quando alguém clica em um link curto, e as decisões de privacidade por trás disso. Se você está decidindo se ativa os headers de geo no seu proxy, ou só quer saber que dado o quark guarda sobre os seus visitantes, esta é a página certa.

## O que é capturado em cada clique

Todo redirect (`GET /:code`) monta um `ClickEvent` a partir dos headers da requisição, de forma fire-and-forget: o redirect em si nunca espera pela analytics. O evento guarda:

| Campo | Origem | Observação |
|---|---|---|
| `country` | header `cf-ipcountry` | Código de duas letras vindo do proxy de borda, não é calculado pelo quark |
| `city` | header `cf-ipcity` | Opcional; vazio na maioria dos deploys (veja abaixo) |
| `referer` | header `Referer` | Valor completo fica no anel de eventos recentes; os agregados agrupam só pelo host |
| `user_agent` | header `User-Agent` | Usado pra derivar device, SO e navegador; a string bruta não aparece nos agregados |
| `ts` | horário do servidor | Timestamp do clique |

A partir desse evento, o quark calcula:

- **Cliques por dia**, pro gráfico de série temporal.
- **Por país** e **por cidade**, a partir dos headers de geo do proxy.
- **Por dispositivo** (Mobile / Desktop / Other), **por SO** (Windows / macOS / iOS / Android / Linux / Other) e **por navegador** (Chrome / Safari / Firefox / Edge / Other), todos por parsing heurístico da string de user agent. Sem base de dados de UA externa, sem dependência nova: mesmo estilo do parser `device_from_ua` já existente.
- **Por referência**, agrupado pelo hostname (`news.ycombinator.com`, `direct` quando não há referrer, `other` quando o referrer não é uma URL válida). Agrupar pelo host, e não pela URL completa, evita que essa quebra cresça sem limite de buckets.

## Postura de privacidade

**O quark nunca armazena um endereço IP.** Nem no backend LMDB, nem no ClickHouse, nem em memória além da própria requisição que está sendo tratada no momento. País e cidade vêm de um header que o proxy de borda já calculou (`cf-ipcountry`, `cf-ipcity`); o quark só lê esse header e segue. Não existe base GeoIP, não existe lookup de IP pra localização, não existe dependência que precisasse de uma.

O que o quark mantém:

- **Agregados**: contadores por dia, país, cidade, dispositivo, SO, navegador e host de referência. São só números; não é possível voltar deles até uma visita específica.
- **Um anel limitado de eventos recentes**: as últimas N linhas de `ClickEvent` bruto por link. O backend LMDB guarda no máximo `EVENTS_MAX` (1000) por link, descartando os mais antigos quando enche; o backend ClickHouse aplica um `LIMIT` na mesma consulta. É isso que alimenta a tabela "Eventos recentes" na tela de estatísticas, com os mesmos campos acima, sem IP entre eles.

Se você não envia `cf-ipcity` (ou não está atrás de um proxy que define esse header), `per_city` simplesmente fica vazio, e a interface esconde esse gráfico em vez de mostrar um gráfico vazio. A maioria dos setups auto-hospedados cai nesse caso: cidade é opcional, não uma expectativa padrão.

## Como ativar os headers de geo

O quark lê dois headers quando eles existem; ele nunca faz o lookup por conta própria:

- `cf-ipcountry`: definido automaticamente pela Cloudflare em toda requisição que passa pela rede deles (veja [`docs/EDGE.PT_BR.md`](EDGE.PT_BR.md) pra como o quark fica atrás da Cloudflare). Nenhuma configuração extra é necessária depois que você está atrás da Cloudflare.
- `cf-ipcity`: **não** vem habilitado por padrão no plano gratuito da Cloudflare. Ativar exige um plano pago com o [managed transform "Add visitor location headers"](https://developers.cloudflare.com/rules/transform/managed-transforms/reference/#add-visitor-location-headers) ligado (Rules → Transform Rules → Managed Transforms, ou a chamada de API equivalente).

Se você está atrás de outro proxy (nginx, Traefik, outro CDN), defina os headers equivalentes na borda e o quark vai captá-los do mesmo jeito, já que o nome do header é a única coisa que ele depende. Não há vendor lock-in no código de analytics: qualquer proxy que envie `cf-ipcountry` / `cf-ipcity` (ou headers renomeados pra combinar) funciona.

## O que está fora do escopo (por ora)

- Uma base GeoIP pra lookup de IP pra cidade sem depender de header de proxy. Isso adicionaria uma dependência pesada e um arquivo de dados pra manter atualizado; o caminho por header já cobre cidade pra quem roda atrás de uma borda que suporta isso.
- Filtro de bots e crawlers. Hoje a analytics conta toda requisição que chega ao redirect, incluindo bots. Filtrar isso é um trabalho separado, pra depois.
- Detalhe completo de referrer por URL. Os agregados agrupam pelo host; o referrer bruto ainda aparece por evento no anel de eventos recentes, se você precisar da URL exata.
