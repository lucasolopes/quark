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

## Filtro de bots

O quark sinaliza cliques prováveis de bots e crawlers a partir da string `User-Agent` e mantém eles fora das quebras acima, mas ainda conta eles de forma honesta.

`is_bot` é uma heurística sem dependência nova, no mesmo estilo dos parsers `device_from_ua` e `os_from_ua`: ela procura substrings comuns de crawler, monitor e biblioteca num User-Agent em minúsculas, coisas como `bot`, `crawler`, `spider`, `crawl`, `slurp`, `bingpreview`, `facebookexternalhit`, `embedly`, `curl`, `wget`, `python-requests`, `httpie`, `go-http-client`, `axios`, `headless`, `phantomjs`, `preview`, `monitor`, `uptime` e `pingdom`. Um User-Agent vazio ou ausente também é tratado como bot: nenhum navegador de verdade manda uma requisição sem esse header.

Isso é uma heurística, não uma certeza. Ela pega crawlers bem comportados e bibliotecas HTTP comuns que se identificam, e vai deixar passar um bot que finge ser um navegador normal. Pense nos números abaixo como "bots potenciais", não uma garantia.

O que isso significa pros números que você vê:

- **`total`** nos agregados continua honesto: conta todo clique que chegou no redirect, bot ou não.
- **`bots`** é um contador separado: quantos desses cliques foram sinalizados. A tela de estatísticas mostra ele do lado do total, como "Bots (excluídos)".
- **Todas as outras quebras** (`per_day`, `per_country`, `per_device`, `per_os`, `per_browser`, `per_referer`, `per_city`) são calculadas só com cliques humanos. Um clique sinalizado incrementa `bots` e é ignorado em todo o resto, então os gráficos refletem visitantes de verdade, não scrapers martelando um link.
- **Eventos recentes** ainda listam todo clique, bot ou não, cada um marcado com uma flag `bot`; a interface mostra um badge pequeno nas linhas sinalizadas pra você conseguir diferenciar sem perder o feed bruto.

O filtro de bots afeta só a analytics. Ele não bloqueia nada: uma requisição sinalizada ainda recebe o redirect normalmente.

## Como o sink do Postgres guarda os cliques

O sink de analytics do Postgres mantém dois tipos de estado, os dois pensados para continuar rápidos quando um link vira febre:

- **Contadores atômicos** (`click_counters`): uma linha por link, dimensão e balde (`total`, `bots` e cada chave de `per_day` / `per_country` / `per_device` / `per_os` / `per_browser` / `per_referer` / `per_city` / `per_variant`). Um flush calcula o delta do lote e aplica cada contador com `count = count + n` via `INSERT ... ON CONFLICT DO UPDATE`. O incremento é atômico, então dois servidores dando flush no mesmo link quente ao mesmo tempo somam as duas contagens sem perder atualização e sem lock para esperar.
- **Eventos append-only** (`click_events`): cada clique entra como uma linha própria, nunca um read-modify-write de um blob compartilhado. As leituras remontam o agregado na hora a partir dos contadores, e a lista de eventos recentes vem das `EVENTS_MAX` (1000) linhas mais novas por link; as mais antigas são cortadas depois de cada flush.

Isso substitui um design anterior que lia o blob agregado inteiro, aplicava o lote em memória e regravava o blob sob um advisory lock por link. Aquilo serializava todo flush de um link quente e regravava o blob inteiro toda vez. A abordagem de contadores tira o lock e a regravação, então um link quente deixa de ser o gargalo do sink.

Uma ressalva: o incremento do contador não é idempotente. Ele resolve a perda de atualização sob concorrência, mas reprocessar o mesmo lote contaria em dobro. A ingestão do quark hoje é at-most-once (um clique é enfileirado uma vez, nunca reprocessado), então está tudo bem. Se um dia entrar entrega at-least-once, os incrementos precisariam deduplicar pelo `ClickEvent.event_id` (uma tabela `processed_events`); isso é um follow-up à parte, fora desta mudança.

Para volume muito alto, o ClickHouse continua sendo o sink recomendado: é colunar, feito para esse tipo de consulta, e o caminho do Postgres é para deploys menores que preferem não rodar um segundo banco.

## Postura de privacidade

**A analytics de clique nunca armazena um endereço IP.** Nem no backend LMDB, nem no ClickHouse, nem no `ClickEvent` ou nos agregados que ele alimenta. País e cidade vêm de um header que o proxy de borda já calculou (`cf-ipcountry`, `cf-ipcity`); o quark só lê esse header e segue. Não existe base GeoIP, não existe lookup de IP pra localização, não existe dependência que precisasse de uma.

Isso vale só pro caminho da analytics de clique. O rate limiter opcional (`src/abuse/ratelimit.rs`, `POST /`) é um mecanismo separado de proteção contra abuso: ele mantém o IP de quem chamou de forma transitória, em memória ou no Valkey, sob uma chave como `quark:rl:{ip}:{window}`, por cerca de um minuto (a janela do rate limit), depois descarta ou deixa expirar. Esse IP nunca é associado a um evento de clique e nunca chega no armazenamento de analytics.

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
- Detecção de bot por IP ou ASN. O filtro atual é só heurística de User-Agent; ele não olha reputação de IP nem dado de rede.
- Um toggle pra incluir bots nos gráficos de quebra. As quebras são só de tráfego humano por design; um toggle por visualização é um possível trabalho futuro, ainda não implementado.
- Detalhe completo de referrer por URL. Os agregados agrupam pelo host; o referrer bruto ainda aparece por evento no anel de eventos recentes, se você precisar da URL exata.
