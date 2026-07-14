[English](README.md) · **Português**

# quark

[![CI](https://github.com/lucasolopes/quark/actions/workflows/ci.yml/badge.svg)](https://github.com/lucasolopes/quark/actions/workflows/ci.yml)
![License: AGPL v3](https://img.shields.io/badge/license-AGPL--3.0-blue)
![Rust 2021](https://img.shields.io/badge/rust-2021-orange)
![Runtime deps: none](https://img.shields.io/badge/runtime%20deps-none-brightgreen)
![Self-hosted](https://img.shields.io/badge/self--hosted-single%20binary-informational)

> Os short codes são **calculados, não armazenados** — uma bijeção chaveada. Um binário estático minúsculo (~1 MB), sem Redis, sem banco de dados, sem serviços externos.

**Links rápidos:** [Deploy](docs/DEPLOY.PT_BR.md) · [Arquitetura](docs/ARCHITECTURE.PT_BR.md) · [Edge/CDN](docs/EDGE.PT_BR.md) · [Roadmap](docs/ROADMAP.PT_BR.md)

Um encurtador de URL cujo short code é uma **permutação ARX de rounds reduzidos e calibrada** do id inteiro interno. O código não é procurado em um índice — ele é **calculado**, nas duas direções, a partir de uma função bijetora minúscula. Essa única decisão de design remove uma classe inteira de problemas (colisões) e um índice inteiro (string → id) de uma vez.

## O pitch

A maioria dos encurtadores escolhe um de dois caminhos para o código:

- **Codificação reversível** (estilo Hashids, Sqids): rápida, mas não é segurança — os códigos são parcialmente enumeráveis. Você consegue raspar `/aaaa`, `/aaab`, …
- **Cifra de verdade** (ex.: Feistly = Feistel + HMAC-SHA256): não enumerável, mas lenta — um hash criptográfico completo roda em cada round.

O quark fecha essa lacuna com uma **rede Feistel cuja função de round é ARX** (add-rotate-xor), não um hash. Uma rede Feistel sobre um domínio inteiro é uma bijeção por construção: `decode(encode(id)) == id` para todo id, com **zero checagens de colisão necessárias**, nunca. A única questão aberta é *quantos rounds* de mistura são necessários até o output parecer aleatório o suficiente pra resistir a enumeração — e isso aqui não é um palpite, é **medido** (veja a tabela de avalanche abaixo). O resultado é um gerador de código simultaneamente não enumerável *e* aproximadamente uma ordem de magnitude mais rápido que uma abordagem de cifra de verdade (~18× medido contra uma Feistel estruturalmente idêntica com round HMAC — veja o benchmark abaixo), porque rounds ARX são operações inteiras baratas, não chamadas de hash.

Como o código é a permutação do id, o store nunca precisa indexar por string. Ele é chaveado por `u64`, direto num banco mmap'd. Milhões de links ocupam uma fração do que um store indexado por string precisaria.

## Arquitetura

```mermaid
flowchart LR
    C[Client] -->|POST /| API[api axum]
    C -->|GET /:code| API
    API -->|encode/decode| P[permute + codec]
    API --> CA[cache moka]
    CA -->|miss| ST[(store LMDB)]
    P -.no I/O.-> API
```

`permute` (a bijeção Feistel/ARX) e `codec` (integer ↔ base62) são matemática pura — sem I/O, sem locks, fora do caminho da requisição. O caminho quente é: decodificar o código pra um id, checar o cache em memória, cair pra uma única leitura mmap em caso de miss.

## Sequência de redirect

```mermaid
sequenceDiagram
    participant Cli as Client
    participant Api as api
    participant Ca as cache
    participant St as store
    Cli->>Api: GET /:code
    Api->>Api: id = decode(base62⁻¹(code), key)
    Api->>Ca: get(id)
    alt hit
        Ca-->>Api: Record
    else miss
        Ca->>St: get_link(id)
        St-->>Ca: Record | None
        Ca-->>Api: Record | None
    end
    alt found and not expired
        Api-->>Cli: 302 Location: url
    else expired
        Api-->>Cli: 410 Gone
    else not found
        Api-->>Cli: 404
    end
```

Códigos numéricos base62 são resolvidos primeiro, por aritmética pura (mascarada, nunca entra em panic). Só um código que **não** é uma string base62 válida de 7 caracteres dentro da faixa — ou seja, tamanho errado, um caractere inválido, ou um valor maior que `MAX_ID` — cai pra uma busca de alias customizado no store.

### Aliases

Um `alias` customizado não pode ser, ele mesmo, um código base62 válido de 7 caracteres na faixa `0..=MAX_ID`: se fosse, seria indistinguível de um código numérico calculado e ficaria inalcançável (sombreado pelo branch numérico acima). `create` rejeita esses aliases com `400 Bad Request` no momento da criação, antes de alocar um id, então eles nunca chegam a entrar no store.

## Quantos rounds? Medido, não chutado

O número de rounds da permutação Feistel/ARX não é escolhido por intuição — é calibrado com um harness de avalanche/SAC (Strict Avalanche Criterion) (`cargo run --bin calibrate`), um port direto do ferramental de medição de difusão construído para um laboratório de pesquisa de SHA-256. A ideia por trás do SAC é simples: **flip um bit do id de entrada, e numa permutação bem misturada, cerca de metade dos bits de saída deveria flipar, de forma imprevisível.** Se flipar o bit 5 do id sempre flipa os mesmos 3 bits de saída, o código é enumerável. Se flipa ~50% dos bits, em média, não importa qual bit de entrada você flipe, o output parece ruído visto de fora.

Resultado medido, varrendo de 1 a 12 rounds sobre 200.000 amostras aleatórias por round:

```
rounds | avalanche_medio | cobertura(/40)
   1   |     0.1381      |    1
   2   |     0.3622      |   21
   3   |     0.4866      |   40
   4   |     0.5000      |   40   ← ROUNDS escolhido (difusão fecha)
 5..12  |     0.5000      |   40
```

- **avalanche_medio**: fração média de bits de saída que flipam quando um bit de entrada flipa (alvo: exatamente 0.5).
- **cobertura**: o mínimo, entre todos os 40 bits de entrada, de quantos bits de saída distintos aquele único bit de entrada já conseguiu afetar. `40/40` significa que todo bit de entrada consegue influenciar todo bit de saída — difusão completa, sem ponto cego estrutural.

`ROUNDS = 4` é o menor número de rounds em que o avalanche bate exatamente `0.5000` *e* a cobertura é total. O round 3 está perto (`0.4866`) mas ainda não chegou lá. Os rounds 5 a 12 não compram nada — a difusão já fechou, então o quark usa 4 e para, mantendo cada round de runtime que não é necessário para a propriedade que está sendo pago por ele.

## Velocidade: o número de troféu

```
cargo bench --bench permute_bench
```

Medido nesta máquina (criterion, `benches/permute_bench.rs`):

| op | time/op | ops/sec |
|---|---|---|
| `permute::encode` (u64 → u64, o motor de permutação) | ~3.98 ns | ~251.000.000 |
| `permute::decode` (u64 → u64) | ~3.45 ns | ~290.000.000 |

Essa é a permutação bruta. A **operação de produto** é `id → string base62 de 7 caracteres` (e de volta), que adiciona uma `String` alocada no heap por chamada:

| op | time/op | ops/sec |
|---|---|---|
| `encode` (id → código string) | ~45 ns | ~22.000.000 |
| `decode` (código string → id) | ~80 ns | ~12.500.000 |

### Cabeça a cabeça (mesma máquina, mesmo harness criterion)

`benches/compare_bench.rs` mede a mesma classe de operação — *id inteiro → string curta opaca* — para o quark e três abordagens concorrentes reais, sobre ids em `0..2^40`. A que isola a afirmação real do quark é a **`feistel_hmac`**: uma Feistel balanceada idêntica (4 rounds, 40 bits), trocando *só* a função de round de ARX pra HMAC-SHA256 (ou seja, a abordagem de "cifra de verdade" que bibliotecas como a Feistly usam).

| abordagem | encode ops/sec | vs quark | o que é |
|---|---|---|---|
| **quark** (Feistel ARX) | **~22.000.000** | 1× | bijeção chaveada, 7 caracteres fixos, não enumerável |
| hashids (`harsh` 0.2.2) | ~2.950.000 | ~7.5× mais lento | codificação de obfuscação (salt fraco, não chaveada) |
| feistel + HMAC-SHA256 | ~1.230.000 | **~18× mais lento** | mesma estrutura do quark, função de round por hash |
| sqids (`sqids` 0.4.2) | ~680.000 | ~32× mais lento | codificação de obfuscação (sem chave) |

O resumo honesto: contra a cifra **estruturalmente idêntica** (mesma Feistel, chaveada, só a função de round difere), o round ARX do quark é **~18× mais rápido** — porque cada round é um punhado de somas/rotações/xors, não uma invocação de hash criptográfico. Esse é o retorno direto de *medir* o número mínimo de rounds (4) em vez de superprovisionar "por segurança".

**Ressalva de justiça:** sqids e hashids são codificações de obfuscação — elas escondem ids sequenciais mas **não** são primitivas criptográficas chaveadas (sqids não tem chave; o salt do hashids é documentado como não seguro), e elas codificam um domínio de tamanho arbitrário em vez da bijeção fixa de 40 bits → 7 caracteres do quark. Então contra essas duas, os números do quark mostram uma vantagem de velocidade, não uma equivalência de segurança. Só a `feistel_hmac` é uma comparação de segurança comparável de fato. Reproduza tudo isso com `cargo bench --bench compare_bench`.

### Capacidade de redirect (HTTP ponta a ponta)

Os números acima são o gerador de código isolado. Servir um redirect de fato adiciona a stack HTTP + a busca no cache. Duas medições, porque respondem a duas perguntas diferentes.

**Produção, de um cliente externo.** Um deploy real numa VPS na Alemanha (Coolify + Traefik + TLS), testado sob carga com k6 a partir de um cliente no Brasil — o caminho completo que um usuário real percorre. `GET /:code`, sem seguir o 302, então mede o quark, não o destino. Reproduza com `scripts/loadtest.k6.js`.

| usuários concorrentes (VUs) | throughput | mediana | p95 | erros |
|---|---|---|---|---|
| 100 | 374 req/s | 214 ms | 228 ms | **0%** |
| 500 | 1.844 req/s | 218 ms | 235 ms | **0%** |
| 1.000 | 3.399 req/s | 221 ms | 262 ms | **0%** |

Como ler isso: **o throughput escala quase linearmente com a concorrência** (10× os usuários → 9.1× as requisições/s) enquanto a **latência fica essencialmente estável** (+7 ms de mediana de 100 pra 1.000 usuários concorrentes) — a assinatura de um servidor que nunca enfileira. **0 erros em ~225k requisições**, 100% de 302s corretos. Os ~214 ms **não são** do quark: é a ida-e-volta São Paulo↔Alemanha (~200 ms de RTT transatlântico). A latência ociosa era de ~212 ms, então o quark adiciona ~2 ms *mesmo sob 1.000 usuários concorrentes*. Um usuário perto da VPS (ex.: na Europa) vê esses mesmos ~2 ms de trabalho do servidor mais o RTT dele, bem menor (dezenas de ms). O gargalo que batemos foi quanto um único cliente distante conseguia puxar — **não o servidor**; seu teto real é maior e só pode ser encontrado com carga distribuída.

**Proxy de capacidade local (sem rede no meio).** Batendo no container de release numa máquina de desenvolvimento (RTT ≈ 0, caminho quente de cache-hit) com `oha`: **~124k req/s @ 50 conexões** (p50 0.33 ms, p99 1.4 ms) e **~152k req/s @ 200 conexões** (p50 0.90 ms, p99 6.3 ms). Isso foi dentro de uma VM do Docker Desktop (CPU limitada), então é um *piso* na capacidade bruta, não um teto. Ainda assim, ~150k redirects/s são ~13 bilhões/dia — ordens de magnitude além da meta de "milhões/dia".

As duas medições apontam pra mesma conclusão: **o caminho de redirect nunca é o fator limitante.** O que um usuário experimenta é dominado pela distância de rede até a VPS, que é um problema de geografia (resolvido por um edge/CDN na frente), não um problema do quark.

## Backends e escala

A persistência, o cache e o analytics do quark ficam cada um atrás de uma trait (`Store`, `CacheTier`, `AnalyticsSink`). O default é um **binário único zero-dependência**; todo outro backend é **opt-in**, selecionado puramente por qual variável de ambiente está definida. Nada aqui muda o formato do caminho quente — muda o que está do outro lado dele.

- **Default — store LMDB + cache L1 in-process + sink de analytics LMDB embutido.** Nenhum serviço externo, nenhuma configuração. `docker run`, pronto. É o que você tem sem nenhuma das variáveis abaixo definida.
- **Cache L2 — Valkey, via `QUARK_VALKEY_URL`.** Um cache L2 compartilhado entre múltiplas instâncias do quark (o cache L1 `moka` é por processo). Protegido por circuit-breaker e um timeout de 100ms por operação: um Valkey fora do ar ou travado nunca bloqueia um redirect — ele simplesmente cai pra L1/store. *Por quê:* escala horizontal sem cada instância dar cold-start no próprio cache.
- **Store relacional — Postgres, via `QUARK_DATABASE_URL`.** Implementa tanto `Store` (links, aliases, uma sequência de id atômica) quanto `AnalyticsSink`. Não definido → LMDB. *Por quê:* persistência segura pra multi-nó, quando um único arquivo LMDB embutido não pode mais ser a fonte da verdade pra mais de uma instância.
- **Sink de analytics — ClickHouse, via `QUARK_CLICKHOUSE_URL`.** Estilo OLAP de append + agregação por query, pra analytics de cliques de alto volume. Não definido → o store ativo, seja qual for, fornece seu próprio sink embutido. ClickHouse é analytics-only — nunca se torna o store de links. *Por quê:* o volume de eventos de clique pode ser ordens de magnitude maior que o volume de criação de links, e quer um column store, não o mesmo motor que serve os redirects.

Store e AnalyticsSink são selecionados **de forma independente** — ex.: store Postgres + analytics ClickHouse, ou store Postgres + seu próprio analytics embutido, são ambas combinações válidas.

- **Webhooks**: eventos HTTP de saída assinados (`link.created/updated/deleted/expired/clicked`) para qualquer endpoint (Zapier, Make, n8n, Slack, customizado), entrega best-effort com retry, configuração persistida via `Store`. Veja [`docs/WEBHOOKS.PT_BR.md`](docs/WEBHOOKS.PT_BR.md).

O enquadramento: o quark **escala pra baixo até um binário único com zero dependências externas**, e **escala pra cima até uma stack distribuída** (Valkey + Postgres + ClickHouse) **uma peça opt-in por vez** — nunca tudo-ou-nada. Compare isso com encurtadores mais pesados (ex.: Dub) que exigem Postgres + Redis + ClickHouse desde o dia um, mesmo pra uma única instância de baixo tráfego.

```mermaid
flowchart LR
    Cli[Client] -->|GET /:code| Api[api axum]
    Api --> L1[L1 moka in-process]
    L1 -->|miss| L2{L2 Valkey opt-in}
    L2 -->|hit| Api
    L2 -->|miss or unset| Store[(Store: LMDB or Postgres)]
    Store --> Api
    Api -->|302| Cli
    Api -.click event.-> Chan[[channel]]
    Chan --> Worker[analytics worker]
    Worker --> Sink[(Sink: LMDB or ClickHouse)]
```



```bash
# QUARK_KEY is parsed as a DECIMAL u64 (not hex). Generate one:
export QUARK_KEY=$(od -An -N8 -tu8 /dev/urandom | tr -d ' ')
export QUARK_DATA=./data        # LMDB directory, created if missing
export QUARK_ADDR=0.0.0.0:8080  # bind address
cargo run --release
```

Se `QUARK_KEY` não estiver definida — ou não for um `u64` decimal válido (uma string hexadecimal falha silenciosamente no parse) — o quark registra um aviso bem visível e cai pra uma chave de dev fixa no código. Está bem pra teste local, **nunca pra produção**: a chave é o que torna o espaço de códigos imprevisível por instância.

### Exemplos com curl

```bash
# create a short link
curl -X POST localhost:8080/ -H 'content-type: application/json' \
  -d '{"url": "https://example.com/some/very/long/path"}'
# => {"code":"01aB2Cd","url":"https://example.com/some/very/long/path"}

# create with a custom alias and a 1-hour TTL
curl -X POST localhost:8080/ -H 'content-type: application/json' \
  -d '{"url": "https://example.com", "alias": "promo", "ttl": 3600}'

# follow it
curl -i localhost:8080/01aB2Cd   # -> 302 Location: https://example.com/...

# health check
curl localhost:8080/health
```

## Modelo de ameaça — leia isto antes de confiar nisso pra sigilo

A não enumerabilidade do quark é uma **propriedade estatística medida** (avalanche/SAC sobre uma permutação ARX de rounds reduzidos), não uma garantia criptográfica. Ela resiste a raspagem casual e a chute sequencial muito melhor que um contador cru ou uma codificação estilo Hashids, e trocar `QUARK_KEY` remapeia o espaço de códigos inteiro. Mas isso **não é AES**, e **não é** um substituto pra controle de acesso de verdade se o próprio recurso linkado precisa continuar em segredo — trate os códigos como "difíceis de adivinhar por força bruta na prática", não como "criptograficamente secretos". Cada instância deveria rodar com sua própria `QUARK_KEY` aleatória, fora do controle de versão.

## Configuração

Toda variável abaixo é opcional, exceto `QUARK_KEY` em produção. Deixe uma variável de backend sem definir e o quark cai pro default zero-dependência.

| Var | Propósito | Default |
|---|---|---|
| `QUARK_KEY` | Segredo `u64` decimal — a chave da permutação. **Obrigatória em produção** (cada instância deveria ter a sua, fora do controle de versão). | chave de dev fallback (aviso bem visível no log; não use em produção) |
| `QUARK_DATA` | Diretório de dados do LMDB. Só usada quando o store é LMDB. | `./data` (container: `/data`) |
| `QUARK_ADDR` | Endereço de bind do HTTP. | `0.0.0.0:8080` |
| `QUARK_ADMIN_TOKEN` | Habilita os endpoints admin protegidos por token: `GET /:code/stats` e `GET/POST/DELETE /admin/blocklist`. | não definida → esses endpoints desligados (404) |
| `QUARK_VALKEY_URL` | Habilita o cache L2 Valkey, ex.: `redis://host:6379`. | não definida → só L1 + store |
| `QUARK_DATABASE_URL` | Usa Postgres pro store, ex.: `postgres://user:pass@host:5432/db`. | não definida → LMDB |
| `QUARK_CLICKHOUSE_URL` | Usa ClickHouse pro analytics, ex.: `http://user:pass@host:8123/db`. | não definida → sink embutido do store |
| `QUARK_ACCESS_LOG` | Habilita o log de acesso JSON por requisição no stdout. | não definida → desligado |
| `QUARK_RATELIMIT_PER_MIN` | Criações/min por IP em `POST /` (não definida/`0` = desligado). Usa Valkey se `QUARK_VALKEY_URL` estiver definida (limite global), senão em memória por réplica. | não definida → desligado |
| `QUARK_REAL_IP_HEADER` | Header de onde ler o IP do cliente. | `CF-Connecting-IP` |
| `QUARK_BLOCK_PRIVATE` | Guarda contra destinos internos/loop; ligada por default, `0` desabilita. | ligada |
| `QUARK_PUBLIC_HOST` | O próprio host desta instância, pro anti-loop (senão usa o header `Host`). | não definida → usa o header `Host` |
| `QUARK_BLOCKLIST_TTL` | Segundos que o snapshot da blocklist fica em cache. | `60` |
| `QUARK_CORS_ORIGINS` | Origens separadas por vírgula com permissão de chamar a API (pro painel web). | não definida → sem CORS (só mesma origem) |

> Só habilite `QUARK_RATELIMIT_PER_MIN` atrás de um proxy que sobrescreve `QUARK_REAL_IP_HEADER` (ex.: Cloudflare com `CF-Connecting-IP`); exposto direto, um cliente pode forjar o header e contornar o limite.

## Operando

- O log de acesso por requisição é **opt-in via `QUARK_ACCESS_LOG`** (desligado por default). Quando definido, cada requisição emite uma **linha de log JSON estruturada** no stdout (`{"method","path","status","latency_ms"}`) — capturada como está pelo Coolify/Docker, prontinha pra `grep` ou enviar a um coletor de logs. Desligado por default pra que o caminho quente de redirect não pague nenhum custo síncrono de `println!`/lock de stdout em alto throughput.
- Redirects carregam um header **`Cache-Control` consciente do TTL**, então um CDN/browser consegue cachear o 302 (e nunca além da expiração de um link). Veja [`docs/EDGE.PT_BR.md`](docs/EDGE.PT_BR.md) pra colocar a Cloudflare na frente.
- A blocklist de domínios é gerenciada via `GET/POST/DELETE /admin/blocklist` (corpo JSON `{"domain": "..."}` pra POST/DELETE), protegida por `QUARK_ADMIN_TOKEN` (header `x-admin-token`; não definida → 404, token errado → 401).

### Stack de dev local

`docker compose up --build` sobe o quark mais os três backends opcionais
(Postgres, Valkey, ClickHouse) já conectados — útil pra desenvolvimento, pra
rodar os testes de integração gated, e como referência de self-host full-stack.
A API admin/painel vive sob `/admin/*` (token `QUARK_ADMIN_TOKEN`): listar
links `GET /admin/links`, apagar `DELETE /admin/links/:code`, editar
`PATCH /admin/links/:code`. Um painel web separado (SPA) consome essa API; configure
`QUARK_CORS_ORIGINS` pra origem do painel.

### Painel web (`web/`)

Um painel admin de operador único (SPA React) vive em `web/`. Ele é construído e
deployado **separadamente** do binário da API (build estático → CDN/edge); o binário
do quark continua API-only. Dev: `cd web && npm install && npm run dev` (Vite na
`:5173`), apontando `VITE_API_BASE_URL` pra sua API do quark e definindo
`QUARK_CORS_ORIGINS=http://localhost:5173` na API. A autenticação é o mesmo
`QUARK_ADMIN_TOKEN`, digitado na tela de login do painel.

## Mais

- Deploy numa VPS com Coolify (traz um `Dockerfile`): [`docs/DEPLOY.PT_BR.md`](docs/DEPLOY.PT_BR.md)
- Guia de cache Edge/CDN: [`docs/EDGE.PT_BR.md`](docs/EDGE.PT_BR.md)
- Webhooks de saída assinados (eventos, payload, verificação de assinatura): [`docs/WEBHOOKS.PT_BR.md`](docs/WEBHOOKS.PT_BR.md)
- O que vem a seguir: [`docs/ROADMAP.PT_BR.md`](docs/ROADMAP.PT_BR.md)
- Design de sistema completo: [`docs/specs/2026-07-12-quark-design.md`](docs/specs/2026-07-12-quark-design.md)
- Passo a passo mais profundo de cada componente, modelo de dados e os internos do round Feistel: [`docs/ARCHITECTURE.PT_BR.md`](docs/ARCHITECTURE.PT_BR.md)
- Escala horizontal (réplicas + Postgres) e o `QUARK_NODE_ID`: [`docs/SCALING.PT_BR.md`](docs/SCALING.PT_BR.md)

## Contribuindo

Contribuições são bem-vindas — veja [`CONTRIBUTING.PT_BR.md`](CONTRIBUTING.PT_BR.md). PRs exigem um [Contributor License Agreement](CLA.PT_BR.md) único (uma concessão de licença — **você mantém a propriedade das suas contribuições**).

## Licença

O núcleo do quark é **AGPL-3.0-only** — veja [`LICENSE`](LICENSE). Copyright © 2026 Lucas Olopes.

- **Self-hosting de uma instância de conta única é livre e sem restrições.**
- Se você rodar um quark **modificado** como serviço de rede pra outros, a AGPL exige que você publique suas modificações sob a mesma licença.
- A edição **cloud multi-tenant** hospedada (contas, cobrança, isolamento de tenant) é uma oferta proprietária separada, não parte deste núcleo AGPL.
- **Licenças comerciais** do núcleo — pra usá-lo sem as obrigações de copyleft da AGPL — estão disponíveis sob solicitação.
