[English](DEVELOPMENT.md) · **Português**

# Desenvolvimento

Como compilar, rodar e testar o quark localmente. O backend é Rust (axum +
tokio); o painel de admin é uma SPA React + Vite em `web/`, compilada e
publicada à parte do binário.

## Pré-requisitos

Uma toolchain Rust stable via [rustup](https://rustup.rs); o
`rust-toolchain.toml` fixa o canal `stable`, então o rustup escolhe sozinho.
Para o painel, Node e npm. Para os testes de integração gated, Docker (ou seu
próprio Postgres, Valkey e ClickHouse).

## Compilar e rodar

```bash
cargo build                 # build de debug
cargo build --release       # binário de release em target/release/quark

# rodar contra um store LMDB local no 0.0.0.0:8080 default
export QUARK_KEY=$(od -An -N8 -tu8 /dev/urandom | tr -d ' ')
cargo run --release
```

Sem nenhuma variável de backend setada, o quark usa o store LMDB embutido, o
cache L1 em processo e o sink de analytics embutido: nenhum serviço externo.
Veja [CONFIGURATION](CONFIGURATION.PT_BR.md) para cada variável.

O binário de calibração offline que mede a difusão do Feistel e escolhe o número
de rounds é separado do serviço:

```bash
cargo run --bin calibrate
```

## A stack completa local

O `docker-compose.yml` sobe o quark mais os três backends opcionais ligados
entre si, espelhando um deploy multi-nó completo numa máquina só:

```bash
docker compose up --build
```

| Serviço | Imagem | Porta |
|---|---|---|
| quark | build do `Dockerfile` do repo | 8080 |
| postgres | `postgres:16` | 5432 |
| valkey | `valkey/valkey:8` | 6379 |
| clickhouse | `clickhouse/clickhouse-server:24` | 8123 |

O serviço `quark` do compose seta `QUARK_DATABASE_URL`, `QUARK_VALKEY_URL`,
`QUARK_CLICKHOUSE_URL`, uma `QUARK_KEY` de dev, um `QUARK_ADMIN_TOKEN` de dev e
`QUARK_CORS_ORIGINS` para o painel. A chave e o token de dev são só para uso
local. Essa stack também é a referência para rodar os testes de integração
gated.

## Testes

Testes unitários ficam inline em módulos `#[cfg(test)]`; testes de integração
são `tests/*_it.rs`. A suíte default não precisa de serviço externo:

```bash
cargo test                                   # testes de lib + API + unitários
cargo fmt --all
cargo clippy --all-targets -- -D warnings    # a CI força -D warnings
```

### Testes gated de backend

Os testes de integração de Postgres, Valkey e ClickHouse são pulados a menos que
a URL correspondente esteja setada. Eles leem um conjunto separado de variáveis
para nunca apontarem para um deploy real por acidente:

| Variável | Habilita |
|---|---|
| `QUARK_TEST_DATABASE_URL` | Testes de store Postgres, analytics, busca, outbox de webhook, escala horizontal |
| `QUARK_TEST_VALKEY_URL` | Testes da camada L2 Valkey e do pub/sub de invalidação |
| `QUARK_TEST_CLICKHOUSE_URL` | Testes do sink ClickHouse |

Aponte para os serviços do compose:

```bash
export QUARK_TEST_DATABASE_URL=postgres://quark:quark@localhost:5432/quark
export QUARK_TEST_VALKEY_URL=redis://localhost:6379
export QUARK_TEST_CLICKHOUSE_URL=http://localhost:8123
```

Esses testes compartilham um banco e o resetam entre casos. Dentro de um binário
de teste os marcadores `#[serial(pg)]` / `#[serial(ch)]` (do `serial_test`) já
impedem testes do mesmo backend de se sobreporem. Entre binários, o cargo roda
os executáveis de teste em paralelo por default, então rode a suíte gated um
binário por vez para dois não truncarem o banco compartilhado um sob o outro:

```bash
cargo test -- --test-threads=1
# ou rode um arquivo gated só
cargo test --test postgres_store_it -- --test-threads=1
```

## Painel web

```bash
cd web
npm install
npm run dev        # servidor de dev do Vite na :5173
npm run test       # Vitest
npm run build      # build estático para CDN/edge
```

Aponte `VITE_API_BASE_URL` para a API do quark rodando e seta
`QUARK_CORS_ORIGINS=http://localhost:5173` na API para o navegador poder chamá-la.
A auth é o mesmo `QUARK_ADMIN_TOKEN`, digitado na tela de login do painel.

## Benchmarks

Os benches Criterion ficam em `benches/`:

```bash
cargo bench --bench permute_bench     # o gerador de código Feistel/ARX isolado
cargo bench --bench compare_bench     # quark vs hashids / sqids / HMAC-Feistel
cargo bench --bench redirect_bench    # o caminho quente do redirect
```

## Onde as coisas estão

O mapa de módulos, os seams de backend e o caminho quente do redirect estão em
[ARCHITECTURE](ARCHITECTURE.PT_BR.md). Formatos de deploy e seus limites em
[SCALING](SCALING.PT_BR.md). O `CONTRIBUTING.md` cobre o CLA e o que se espera
de um PR.
