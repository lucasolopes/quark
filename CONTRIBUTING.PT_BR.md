[English](CONTRIBUTING.md) · **Português**

# Contribuindo com o quark

Obrigado pelo interesse. O quark é open source sob a **GNU AGPLv3** (veja
[`LICENSE`](LICENSE)). Contribuições de código, documentação, testes e relatos
de bug são bem-vindas.

Ao participar você concorda com o [Código de Conduta](CODE_OF_CONDUCT.PT_BR.md).

## Contributor License Agreement (obrigatório)

Antes do seu pull request ser mergeado, você precisa aceitar o
[Contributor License Agreement](CLA.PT_BR.md). É uma **concessão de licença, não
uma transferência de copyright**: **você continua dono das suas contribuições**.
Você concede ao mantenedor uma licença ampla (incluindo o direito de
relicenciar) para que o quark possa ser oferecido sob a AGPL e, separadamente,
sob licença comercial e numa edição hospedada. É o mesmo modelo de Dub, n8n e
Grafana.

Assinar é **um clique, uma vez só**: no seu primeiro PR um bot posta o link;
aceite uma vez e vale para todos os PRs seguintes.

## Formas de contribuir

- **Dúvidas e ajuda para subir o projeto** vão para
  [Discussions, Q&A](https://github.com/lucasolopes/quark/discussions/categories/q-a),
  não para o issue tracker.
- **Bugs** vão no [formulário de bug](https://github.com/lucasolopes/quark/issues/new?template=bug.yml),
  com reprodução contra uma instância limpa.
- **Problemas de segurança** nunca em público. Veja
  [SECURITY.PT_BR.md](SECURITY.PT_BR.md).
- **Pegar uma tarefa**: issues com label `good first issue` e `help wanted`
  estão livres. Comente na issue antes, para duas pessoas não escreverem o mesmo
  patch.

## Desenvolvimento

Pré-requisitos: toolchain estável do Rust (via [rustup](https://rustup.rs)) e
Node 20+ para o painel. O detalhe está em
[docs/DEVELOPMENT.PT_BR.md](docs/DEVELOPMENT.PT_BR.md); aqui fica só o mínimo
para o primeiro PR.

Backend:

```bash
cargo build
cargo test          # testes de lib + API, sem serviços externos
cargo fmt --all
cargo clippy --all-targets -- -D warnings
```

Painel admin (`web/`, React + TypeScript + Vite):

```bash
cd web
npm ci
npm run dev
npm run lint        # oxlint, --max-warnings 0
npm run typecheck   # tsc -b, esse aqui já quebrou deploy
npm run test        # Vitest
npm run build
```

Os testes de integração de Postgres, Valkey e ClickHouse dependem de
`QUARK_TEST_DATABASE_URL`, `QUARK_TEST_VALKEY_URL` e
`QUARK_TEST_CLICKHOUSE_URL`, e são pulados quando as variáveis não estão
definidas. A maioria das mudanças não precisa deles.

## Testes

- Superfície de API: testes de integração em `tests/*_it.rs`. Monte o
  `AppState` pelo builder `TestState` em `tests/common/mod.rs`, não com struct
  literal na mão.
- Unidade: módulos `#[cfg(test)]` inline, ao lado do código.
- Painel: `web/src/**/*.test.tsx` com Vitest.
- Mantenha o **caminho quente de redirect** com poucas alocações. É o caminho
  crítico de performance, veja [`benches/redirect_bench.rs`](benches/redirect_bench.rs).

## Regras de documentação e i18n

Duas regras fáceis de esquecer e que serão pedidas no review:

1. **Toda doc voltada ao usuário tem um gêmeo `.PT_BR.md`.** `docs/WEBHOOKS.md`
   e `docs/WEBHOOKS.PT_BR.md`. Os dois começam com o header de troca de idioma:
   `**English** · [Português](X.PT_BR.md)` e o espelho no gêmeo. Um PR de doc em
   um idioma só está incompleto.
2. **Toda string nova do painel entra em `web/src/i18n/en.ts` e em
   `web/src/i18n/pt-BR.ts`.** Nada de string fixa no componente.

Estilo do texto: inglês técnico direto, sem travessão, e pt-BR natural no
gêmeo. Não traduza ao pé da letra, escreva como se escreve no idioma.

## Dependências novas

"Zero dependências em runtime" e "binário de ~1 MB" são o argumento do projeto.
Uma crate nova precisa de justificativa na descrição do PR: o que ela faz, por
que a std ou uma dependência existente não resolve, e quanto custa em tamanho de
binário. Em `web/` a régua é ainda mais alta, porque o bundle vai para todo
usuário do painel.

## Commits, branches e pull requests

- Branches: `feat/slug-curto`, `fix/slug-curto`, `chore/slug-curto`.
- Commits: [Conventional Commits](https://www.conventionalcommits.org/) com
  escopo, `feat(web):`, `fix(api):`, `docs:`, `chore:`. **Escreva as mensagens
  de commit em inglês.** O histórico antigo está em português e fica como está.
- Atualize o `CHANGELOG.md` **e** o `CHANGELOG.PT_BR.md` em `## [Unreleased]`
  no mesmo PR, não depois. É a mesma regra do gêmeo bilíngue que vale para
  qualquer outro doc voltado ao usuário. O que você escrever ali é o que vai
  sair nas notas da release, então escreva pensando em quem não acompanhou o
  pull request.
- Faça fork e abra o PR contra a `main`.

Mergear não lança nada. Só uma tag do git publica imagem e faz deploy, então sua
mudança fica na `main` até a próxima versão ser cortada. É por isso que a entrada
em `## [Unreleased]` importa: é dela que a release é montada.

Como funciona o gate de merge, para nada te pegar de surpresa:

- O CI precisa passar: `cargo fmt --check`, `cargo clippy -D warnings`,
  `cargo test`, cargo-deny, dependency review, e o job web de
  `lint`/`typecheck`/`test`/`build`.
- É obrigatório 1 review aprovando, e todo thread de review precisa ser
  resolvido.
- **Qualquer push depois de uma aprovação descarta aquela aprovação.** A regra
  do branch é "require last push approval", então corrigir um typo de última
  hora significa pedir review de novo. Agrupe suas mudanças.
- Espere a primeira resposta em até uma semana. Silêncio quer dizer "ainda não
  olhei", não "recusado".

## O que não vai ser mergeado

Para você não perder tempo:

- mudanças no caminho quente de redirect que trocam latência por conveniência
- um backend de store novo sem alguém comprometido em mantê-lo
- dependência de serviço externo no caminho padrão
- reescrita de estilo em massa, reformatação ou renomeação sem relação com uma
  correção
- mudança no esquema de código curto. É o núcleo do projeto e precisa de um
  spec em `docs/specs/` acordado antes de qualquer código.

## Quem decide

O quark tem um mantenedor só, @lucasolopes, que dá a palavra final sobre escopo,
design e o que entra. A direção é pública em
[docs/ROADMAP.PT_BR.md](docs/ROADMAP.PT_BR.md), e os specs de design entram em
`docs/specs/` antes do código, então dá para discordar de uma decisão enquanto
mudá-la ainda é barato.

O [CLA](CLA.PT_BR.md) permite que o projeto seja oferecido sob a AGPL e,
separadamente, sob licença comercial. É uma escolha deliberada, não um passo
para fechar o código: a edição AGPL é o projeto, não uma amostra. Se algum dia
fizer sentido um modelo de governança com mais de um mantenedor, ele será
escrito aqui primeiro.

## Onde estão as coisas

- [`docs/ARCHITECTURE.PT_BR.md`](docs/ARCHITECTURE.PT_BR.md): como as peças se
  encaixam.
- [`docs/DEVELOPMENT.PT_BR.md`](docs/DEVELOPMENT.PT_BR.md): o guia completo de
  desenvolvimento.
- [`docs/ROADMAP.PT_BR.md`](docs/ROADMAP.PT_BR.md): direção e próximos passos.
- [`docs/SCALING.PT_BR.md`](docs/SCALING.PT_BR.md): formatos de deploy e seus
  limites.
