[English](CONFIGURATION.md) · **Português**

# Referência de configuração

Toda configuração do quark é uma variável de ambiente lida uma vez no startup.
Não há arquivo de config nem feature flag de build: quais backends rodam é
decidido só por quais variáveis `QUARK_*` estão setadas. Esta página lista cada
variável que o binário lê, o default e o que faz. A fonte da verdade é
`src/main.rs`, mais `src/cluster.rs` (o preflight de cluster), `src/store/mod.rs`
(escolha de backend) e `src/api.rs` (CORS).

Só `QUARK_KEY` importa para um deploy real. Deixe o resto sem setar e o quark
roda como um binário único sem dependências em `0.0.0.0:8080` com store LMDB.

## Núcleo

| Variável | Default | Função |
|---|---|---|
| `QUARK_KEY` | fallback de dev `11400714819323198485` (aviso alto) | A chave da permutação, lida como `u64` **decimal**. É o que torna o espaço de códigos imprevisível por instância. Use um valor aleatório em produção e mantenha fora do controle de versão. Uma string hex não parseia e cai silenciosamente na chave de dev. |
| `QUARK_SIGNING_KEY` | aleatória por processo (aviso alto) | Segredo base64 (>= 32 bytes) que assina os cookies de unlock de senha, separado do `QUARK_KEY`. Sem setar, gera uma chave aleatória a cada start, então os cookies de unlock não sobrevivem a um restart nem são compartilhados entre nós. Set (e compartilhe entre réplicas) para deploys multi-nó ou persistentes. Só importa se você usa links com senha. |
| `QUARK_HEALTH_CHECK_SECS` | sem setar (desligado) | Liga o monitoramento de link quebrado: segundos entre varreduras de saúde do destino (elevado pra 60 no mínimo). Ainda sem coordenação entre nós, então set em exatamente uma instância. Veja [LINK-HEALTH](LINK-HEALTH.PT_BR.md). |
| `QUARK_ADDR` | `0.0.0.0:8080` | Endereço de bind HTTP. |
| `QUARK_DATA` | `./data` (imagem: `/data`) | Diretório de dados do LMDB, criado se faltar. Só usado quando o store é LMDB (sem `QUARK_DATABASE_URL`). |

Gere uma chave com `od -An -N8 -tu8 /dev/urandom | tr -d ' '`. Trocar
`QUARK_KEY` remapeia todo o espaço de códigos, então todo código já emitido para
de resolver. Mantenha estável depois que houver links.

## Backends

Cada backend é opt-in e escolhido de forma independente. O store segue
`QUARK_DATABASE_URL`; o sink de analytics segue `QUARK_CLICKHOUSE_URL` se
setado, senão é o sink embutido do próprio store; o cache L2 e a conexão de
controle compartilhada seguem `QUARK_VALKEY_URL`.

| Variável | Default | Função |
|---|---|---|
| `QUARK_DATABASE_URL` | sem setar (LMDB) | Usa Postgres como store, ex. `postgres://user:pass@host:5432/db`. É o store compartilhado, seguro para multi-nó, e também implementa o sink de analytics. Sem setar, cai no LMDB embutido. |
| `QUARK_VALKEY_URL` | sem setar (só L1 + store) | Liga o cache L2 no Valkey, ex. `redis://host:6379`. A mesma conexão sustenta o rate limit global e o pub/sub de invalidação cross-node. |
| `QUARK_CLICKHOUSE_URL` | sem setar (sink embutido do store) | Usa ClickHouse como sink de analytics, ex. `http://user:pass@host:8123/db`. É só analytics; nunca vira o store de links. |
| `QUARK_NODE_ID` | sem setar (espaço de id de 40 bits cheio) | Particionamento de espaço de id, só no LMDB, `0`-`255`. Os 8 bits do topo viram o id do nó e os 32 de baixo um contador local. Ignorado no backend Postgres (a sequência compartilhada aloca) e o quark loga que foi ignorado. Um valor fora da faixa aborta o processo no startup. Veja [SCALING](SCALING.PT_BR.md). |

`QUARK_NODE_ID` particiona o espaço de id para códigos nunca colidirem entre nós
LMDB; não faz arquivos LMDB separados compartilharem links. Multi-nó de verdade
precisa de Postgres. O id tem que ser único por réplica e o quark não detecta
duplicata.

## Preflight de cluster

| Variável | Default | Função |
|---|---|---|
| `QUARK_STRICT_CLUSTER` | sem setar (off) | Setada com qualquer valor não vazio, o quark se recusa a subir a menos que `QUARK_DATABASE_URL` e `QUARK_VALKEY_URL` estejam presentes, e nomeia a que faltou. Transforma uma configuração multi-nó silenciosamente errada (arquivos LMDB por nó, rate limit N vezes, caches velhos) em erro de startup. Deixe sem setar para single-node. |

A checagem é `cluster_preflight` em `src/cluster.rs`. Com strict off, sempre
passa e o comportamento single-node fica intacto.

## Admin e acesso

| Variável | Default | Função |
|---|---|---|
| `QUARK_ADMIN_TOKEN` | sem setar | O token do operador, enviado no header `x-admin-token`. Sempre se comporta como escopo `full`. Sem setar, `POST /` fica um encurtador público aberto e todo endpoint `/admin/*` mais `GET /:code/stats` responde `404` (desligado). Setado, esses endpoints exigem ele ou um token de API com escopo, e `POST /` exige um token que cubra `links_write`. Veja [API-TOKENS](API-TOKENS.PT_BR.md). |
| `QUARK_CORS_ORIGINS` | sem setar (só mesma origem) | Lista de origens separadas por vírgula liberadas a chamar a API, para o painel web hospedado à parte. Vazio significa sem camada de CORS. |
| `QUARK_ACCESS_LOG` | sem setar (off) | Liga uma linha de log de acesso em JSON por requisição (`{"method","path","status","latency_ms"}`) no stdout. Off por default para o caminho quente do redirect não pagar custo síncrono de stdout. |

## Proteção contra abuso

Valem só para `POST /`. O caminho do redirect nunca é tocado por elas.

| Variável | Default | Função |
|---|---|---|
| `QUARK_RATELIMIT_PER_MIN` | sem setar / `0` (off) | Criações por minuto por IP no `POST /`, janela fixa de 60s. Com `QUARK_VALKEY_URL` setado é um limite global entre réplicas (Valkey `INCR`/`EXPIRE`); senão é em memória por réplica. Fail-open: um erro do Valkey deixa a requisição passar. |
| `QUARK_REAL_IP_HEADER` | `cf-connecting-ip` | Header de onde ler o IP do cliente, com fallback no endereço do socket. Como o header é confiado, só ligue o rate limit atrás de um proxy que o sobrescreva, ou um cliente pode forjá-lo. |
| `QUARK_BLOCK_PRIVATE` | ligado (setar `0` desliga) | O guard de rede interna/loop. Rejeita destino cujo host seja um IP literal privado, loopback ou link-local (v4 e v6, incluindo IPv4-mapeado como `::ffff:127.0.0.1`), `localhost`, ou o próprio host da instância. Nunca resolve DNS. |
| `QUARK_PUBLIC_HOST` | sem setar (usa o header `Host`) | O host da própria instância, usado pela checagem anti-loop para um link não apontar de volta ao quark. |

## Defaults compilados no binário

São constantes de compilação, não variáveis de ambiente, mas limitam o
comportamento e ajudam a dimensionar um deploy. Todas em `src/main.rs` salvo
indicado.

| Constante | Valor | O que limita |
|---|---|---|
| Capacidade do cache L1 | 100.000 registros | Máx. de entradas `id -> Record` no cache L1 (moka) por processo. |
| TTL do cache L1 | 60s (`src/cache/mod.rs`) | Quanto uma entrada L1 vive antes de recarregar. Também o backstop de defasagem entre nós. |
| TTL do cache L2 | 3600s (`src/cache/mod.rs`) | TTL da entrada L2 no Valkey, reduzido para um link perto de expirar. |
| Timeout de op L2 | 100ms (`src/cache/mod.rs`) | Limite por chamada ao Valkey; um timeout conta como falha do breaker. |
| Capacidade do canal de analytics | 10.000 eventos | `ClickEvent`s bufferizados antes do `try_send` do redirect descartar (ingestão at-most-once). |
| Capacidade do canal de webhook | 1.024 eventos (`src/webhooks/delivery.rs`) | Profundidade da fila best-effort em memória de webhooks. |
| Largura do espaço de id | 40 bits (`src/permute.rs`) | `MAX_ID = 2^40 - 1`, cerca de 1,1 trilhão de links. |
| Teto de linhas de import | 10.000 linhas (`src/import.rs`) | Máx. de linhas por requisição `POST /admin/import`. |

## Páginas relacionadas

- [Deploy no Coolify](DEPLOY.PT_BR.md) mostra essas variáveis num deploy real.
- [Desenvolvimento](DEVELOPMENT.PT_BR.md) cobre a stack Docker local e os testes
  de integração gated nas variáveis `QUARK_TEST_*`.
- [Escala](SCALING.PT_BR.md) explica `QUARK_STRICT_CLUSTER`, `QUARK_NODE_ID` e a
  matriz single-node versus multi-nó.
