[English](CONTRIBUTING.md) · **Português**

# Contribuindo com o quark

Obrigado pelo interesse! O quark é open source sob a **GNU AGPLv3** (veja
[`LICENSE`](LICENSE)). Contribuições de código, docs, testes e reports de bug são
bem-vindas.

## Contributor License Agreement (obrigatório)

Antes que seu pull request possa ser mesclado, você precisa aceitar o
[Contributor License Agreement](CLA.PT_BR.md). É uma **concessão de licença, não uma
transferência de copyright**: **você mantém a propriedade total das suas contribuições**.
Você concede ao mantenedor uma licença amplas (incluindo o direito de relicenciar) para
que o quark possa ser oferecido tanto sob a AGPL quanto, separadamente, sob uma licença
comercial e uma edição hospedada. Esse é o mesmo modelo usado por projetos análogos
neste espaço (Dub, n8n, Grafana).

Assinar é um **clique único**: quando você abre seu primeiro PR, um bot automático
posta um link; aceite uma vez e isso cobre todos os seus PRs futuros.

## Desenvolvimento

Pré-requisitos: um toolchain Rust estável (via [`rustup`](https://rustup.rs)).

```bash
cargo build
cargo test          # lib + API tests — no external services needed
cargo fmt --all
cargo clippy --all-targets -- -D warnings
```

Os testes de integração de Postgres / Valkey / ClickHouse ficam atrás de variáveis
de ambiente (`QUARK_TEST_DATABASE_URL`, `QUARK_TEST_VALKEY_URL`, `QUARK_TEST_CLICKHOUSE_URL`)
e são pulados quando não definidas. Você não precisa desses serviços pra maioria das mudanças.

## Antes de abrir um PR

- `cargo fmt --all` e `cargo clippy --all-targets -- -D warnings` precisam estar limpos
  (a CI força `-D warnings`).
- Adicione ou atualize testes pra qualquer mudança de comportamento. Mantenha o
  **caminho quente de redirect** com poucas alocações: é o caminho crítico de performance
  (veja [`benches/redirect_bench.rs`](benches/redirect_bench.rs)).
- Mantenha as mudanças focadas; explique o quê e o porquê na descrição do PR.
- Para mudanças maiores, abra uma issue primeiro pra alinhar a direção.

## Onde as coisas estão

- [`docs/ARCHITECTURE.PT_BR.md`](docs/ARCHITECTURE.PT_BR.md): como as peças se encaixam.
- [`docs/ROADMAP.PT_BR.md`](docs/ROADMAP.PT_BR.md): direção e o que vem a seguir.
- [`docs/SCALING.PT_BR.md`](docs/SCALING.PT_BR.md): formatos de deploy e seus limites.
