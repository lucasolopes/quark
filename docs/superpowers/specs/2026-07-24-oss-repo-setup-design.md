# Setup do repositório para padrão open source profissional

Data: 2026-07-24
Branch: `chore/oss-repo-setup`
Status: aprovado para planejamento

## Problema

O `lucasolopes/quark` é público, tem licença AGPL-3.0, README forte e CI que roda.
Mas quem chega nele hoje vê um repositório sem descrição, sem topics, sem política
de segurança, sem template de issue, sem nenhuma release e com uma Wiki vazia
habilitada. A leitura imediata é "projeto abandonado ou pessoal", o que não
corresponde ao software.

Existe também um problema real de engenharia por trás da fachada: o ruleset da
`main` não exige status checks. O CI roda, fica vermelho, e o merge acontece do
mesmo jeito.

## Estado verificado em 2026-07-24

Confirmado via `gh api`, não por suposição.

Já existe e está correto:

- `LICENSE` AGPL-3.0, `README.md` + `README.PT_BR.md`, `CONTRIBUTING.md` + twin,
  `CLA.md` + twin com bot de assinatura em `signatures/version1/cla.json`
- `.github/workflows/ci.yml` (fmt, clippy com `-D warnings`, build, testes com
  services de Postgres/Valkey/ClickHouse; job `web` com lint, typecheck, test,
  build; job `deploy-backend` para o Fly)
- `docs/` extenso e bilíngue
- CodeQL default setup ativo: linguagens `rust`, `javascript-typescript`,
  `actions`, suite `default`, agendamento semanal
- Private Vulnerability Reporting ativo
- Secret scanning ativo, Dependabot alerts e security updates ativos
- Ruleset ativo na `main` (id 19673028): `deletion`, `non_fast_forward`,
  `pull_request` com 1 aprovação, dismiss stale, require last push approval,
  thread resolution. Bypass para RepositoryRole 5 (admin), modo `always`
- Labels `dependencies`, `rust`, `javascript` além das padrão
- Issues e Discussions habilitados

Falta:

- Descrição, topics e homepage do repositório: todos vazios
- `required_status_checks` no ruleset. Os contexts que já rodam na `main` são
  `check`, `web`, `Analyze (rust)`, `Analyze (actions)`,
  `Analyze (javascript-typescript)`, `deploy-backend`, `Cloudflare Pages`
- Push protection do secret scanning, non-provider patterns e validity checks
- `SECURITY.md`, `CODE_OF_CONDUCT.md`
- `.github/ISSUE_TEMPLATE/`, `PULL_REQUEST_TEMPLATE.md`, `CODEOWNERS`
- `.github/dependabot.yml`
- `deny.toml` e verificação de licença de dependência, que num projeto AGPL é o
  único mecanismo que avisa sobre incompatibilidade
- `permissions:` declaradas nos workflows
- Actions pinadas por SHA. Hoje `superfly/flyctl-actions/setup-flyctl@master`
  executa código de terceiro no job que carrega `FLY_API_TOKEN`
- Qualquer tag, release ou changelog. `Cargo.toml` parado em `0.1.0`
- Wiki habilitada e vazia
- `delete_branch_on_merge` desligado

## Decisões tomadas

| Questão | Decisão |
| --- | --- |
| Ambição | Setup completo: health files, templates, dependabot, scanning, release e metadados |
| Idioma | Bilíngue, `X.md` + `X.PT_BR.md` com header de troca de idioma, seguindo o padrão do repo |
| Artefatos de release | Somente imagem Docker multiarch no GHCR. Sem binários por plataforma anexados |
| Assinatura de commit | Removida do ruleset em 2026-07-24. Com mantenedor solo só cria atrito no primeiro contribuidor externo e quebra PR de bot |
| Aprovação em PR | Mantém 1 aprovação com bypass de admin. A exigência é real para terceiro e contornável por quem mantém |
| Escopo da entrega | As quatro fases, incluindo as mudanças de código Rust da fase 3 |
| Forma de entrega | Um PR único, a partir de worktree isolado em `chore/oss-repo-setup` |
| Versão da primeira release | `v0.2.0`. A `0.1.0` nunca foi publicada e o software já tem OIDC, multi-tenant, webhooks e ClickHouse. Sair como `0.1.0` passaria impressão de protótipo |

## Fora de escopo

Cortados por YAGNI. Mantenedor solo, 7 stars, pré-1.0. Cada item abaixo custaria
manutenção sem resolver problema que o projeto tenha hoje.

- `GOVERNANCE.md`: descreveria um processo de decisão nunca exercido. Seis linhas
  no CONTRIBUTING dizendo quem decide resolvem
- `SUPPORT.md`: o `config.yml` dos issue templates intercepta a pessoa no
  momento do clique errado, que é onde um arquivo na sidebar não chega
- `docs/VERSIONING.md`: a convenção de 0.x cabe no cabeçalho do CHANGELOG
- Alias de e-mail para segurança e conduta: o PVR já cobre vulnerabilidade e o
  `github.com/contact/report-abuse` cobre conduta. Nenhuma caixa nova para manter
- `question.yml`: vira contact link para Discussions
- Workflow próprio de CodeQL: o default setup já está ativo e um workflow
  avançado dá conflito no upload de SARIF
- `cargo-audit`: o `cargo-deny` usa a mesma base RustSec e ainda checa licença
- Job de `npm audit`: Dependabot alerts e dependency-review já cobrem, e ele
  gera falso positivo em devDependency
- `cosign`: a attestation de build do GitHub dá a mesma garantia via Sigstore,
  sem um segundo formato para documentar
- `release-please`, `git-cliff` e `cargo-dist`: gerariam changelog a partir de
  commits em pt-BR e inglês misturados, com ID do Linear vazando, e colidiriam
  com o requisito bilíngue
- Badge do OpenSSF Scorecard enquanto a nota estiver abaixo de 7.5. Badge de
  nota baixa num projeto que vende segurança é pior que badge nenhum
- `FUNDING.yml` e homepage: só quando existir a landing ou o GitHub Pages.
  Apontar homepage para a pasta `docs` é link circular

## Arquitetura da entrega

Quatro fases num PR único. A ordem interna importa porque há dependências reais
entre os passos, e alguns passos são configuração de repositório via API, que
não vive no PR e precisa acontecer em momento certo em relação ao merge.

### Fase 1: metadados e health files

Nenhuma mudança de código. Entrega o repositório deixando de parecer abandonado
e a triagem passando a ser estruturada.

Configuração de repositório, via `gh`:

- Descrição com os termos de busca nas primeiras palavras (self-hosted, URL
  shortener, Rust) e a afirmação que nenhum concorrente pode fazer no fim (o
  código é computado, não armazenado)
- Vinte topics, que é o limite do GitHub. `quark` é um nome colidido, então os
  topics carregam a descoberta inteira
- Wiki desligada
- Push protection, non-provider patterns e validity checks do secret scanning
- `delete_branch_on_merge` ligado
- Categorias Q&A e Ideas criadas em Discussions, antes de o `config.yml` entrar

Arquivos, todos bilíngues:

- `SECURITY.md`: aponta para o formulário de advisory do PVR, declara versões
  suportadas de forma honesta para pré-1.0 (só a última minor) e define escopo
  do que é e do que não é vulnerabilidade. SLA realista de mantenedor solo
- `CODE_OF_CONDUCT.md`: Contributor Covenant 2.1 verbatim, contato via
  report-abuse do GitHub
- `.github/ISSUE_TEMPLATE/bug.yml` e `feature.yml`: forms YAML, não markdown
  legado. O bug form pergunta versão, backend de store, cache e analytics,
  porque com três backends plugáveis é isso que salva a triagem
- `.github/ISSUE_TEMPLATE/config.yml`: `blank_issues_enabled: false` e contact
  links para Discussions Q&A, Ideas, security advisory e docs
- `.github/PULL_REQUEST_TEMPLATE.md`: só o que o CI e o bot de CLA não checam
- `.github/CODEOWNERS`: duas linhas, para auto-request de review em PR externo.
  A regra "require code owner review" não será ligada
- `CONTRIBUTING.md` reescrito: o atual tem 51 linhas e não menciona o frontend
  em `web/`, i18n, a regra bilíngue de docs, convenção de commit, o que não
  será mergeado nem quem decide
- Correção do bloco de código órfão no README, que hoje aparece sem heading

### Fase 2: automação e supply chain

Entrega dependências se atualizando sozinhas e nada entrando sem passar por
licença e advisory.

A ordem aqui é obrigatória. O `cla.yml` precisa ser blindado contra o Dependabot
antes de o `dependabot.yml` existir, senão todo PR do bot nasce com check
vermelho.

1. `if: github.actor != 'dependabot[bot]'` no job do `cla.yml`
2. `ci.yml` reescrito: `permissions:` no topo, actions pinadas por SHA, e
   `deploy-backend` passando a depender de `[check, web]`. Hoje ele depende só
   de `check`, ou seja, deploya com o front quebrado
3. Label `github-actions` criada
4. `.github/dependabot.yml` com cargo, npm e actions. Agrupamento agressivo,
   porque `require_last_push_approval` combinado com `dismiss_stale_reviews_on_push`
   faz a aprovação sumir a cada rebase do bot. Poucos PRs, revisados em lote.
   Os ecossistemas docker ficam de fora até o Dockerfile estar pinado por digest
5. `deny.toml` com as duplicatas reais preenchidas depois de rodar local, mais
   `.github/workflows/supply-chain.yml`
6. `.github/workflows/dependency-review.yml`, que cobre npm e actions, que o
   cargo-deny não enxerga

O `required_status_checks` no ruleset acontece **depois do merge**, não no PR.
Os contexts precisam ter rodado ao menos uma vez na `main` para poderem ser
referenciados. Adicionar um context com nome errado deixa todo PR preso em
"Expected, waiting for status" para sempre, com saída só por bypass de admin.

### Fase 3: release Docker no GHCR

Aqui muda código Rust, não só configuração.

1. `--version` no binário e campo `version` no `/healthz`. O guard do release e
   o campo de versão do bug form dependem disso
2. `Cargo.toml` para `0.2.0`, com `repository`, `homepage`, `readme`, `keywords`
   e `categories`
3. `Dockerfile` reescrito com cargo-chef, imagens base pinadas por digest e
   `ca-certificates`. Confirmar antes se o `reqwest` usa `webpki-roots` embutido
   ou o store do sistema, porque sem isso webhook de saída e discovery de OIDC
   falham com erro que parece problema de rede
4. `.dockerignore` reescrito. Hoje o `node_modules/` da raiz entra no contexto e
   invalida a camada de build
5. `CHANGELOG.md` e twin, formato Keep a Changelog, com o parágrafo de semântica
   0.x no cabeçalho. O heading da seção precisa ser exatamente
   `## [0.2.0] - AAAA-MM-DD`, porque o workflow extrai as notas dele
6. Ruleset de tag para `v*`, criado via API **antes** de o `release.yml` ser
   mergeado. Rulesets de branch não cobrem tags, e o `release.yml` tem
   `packages: write`
7. `.github/workflows/release.yml`: dispara em tag `v*`, job guard comparando a
   tag com o `Cargo.toml`, build multiarch nativo em matrix (`ubuntu-latest` e
   `ubuntu-24.04-arm`, ambos grátis em repo público, portanto sem QEMU), cache
   do buildx com scope por arquitetura, merge do manifest, attestation de
   provenance aplicada ao índice e não ao manifest por arquitetura, e criação da
   Release com as notas extraídas do CHANGELOG
8. Tag `v0.2.0` empurrada, package do GHCR tornado público pela UI, e
   verificação anônima do pull e da attestation

### Fase 4: descoberta

Entrega o leitor entendendo o produto em vinte segundos.

- Screenshot do painel em `docs/assets/`, referenciado no topo do README
- `## Quick start` movido para logo abaixo dos quick links, com o `docker run`
  real que passa a existir depois da fase 3
- Tabela comparativa contra Shlink, YOURLS, Kutt e Dub, com a coluna "serviços
  exigidos: nenhum" visível
- Tabela de avalanche anglicizada no README em inglês, onde hoje vaza pt-BR
- Máquina dos benchmarks especificada. Nenhuma tabela deve dizer "measured on
  this machine" sem dizer qual
- Badges reduzidos a no máximo cinco, nenhum estático escrito à mão. License
  vira dinâmico, entram release e GHCR, saem "Rust 2021" e um dos redundantes
- Tabela de configuração inline reduzida de 17 para 6 variáveis, com link para
  `docs/CONFIGURATION.md`
- Social preview 1280x640, com o conteúdo dentro de 1120x520

Divulgação e submissão ao awesome-selfhosted ficam para depois do merge e são
tarefa manual do dono. O awesome-selfhosted conta quatro meses a partir do
primeiro release, não do primeiro commit, e o repositório nasceu em 12/07/2026.

## Riscos conhecidos

Os que têm consequência real e não são óbvios no momento de executar.

1. **`cla.yml` quebrando em PR do Dependabot.** Em evento disparado por bot o
   `GITHUB_TOKEN` é rebaixado para read-only e os secrets não são expostos. O
   workflow usa `secrets.PERSONAL_ACCESS_TOKEN` e chama `issues.createComment`,
   o que dá 403. Um `allowlist` resolveria a checagem de CLA mas não impediria o
   job de rodar e falhar. Por isso o `if:` no nível do job, e por isso ele vem
   antes do `dependabot.yml`
2. **`required_status_checks` aplicado cedo demais trava o repositório.** Os
   nomes precisam sair de `gh api repos/lucasolopes/quark/commits/main/check-runs`
   depois do merge, copiados exatamente
3. **`superfly/flyctl-actions/setup-flyctl@master`.** Qualquer commit no master
   da Superfly roda no runner que tem o `FLY_API_TOKEN` de produção. É o mesmo
   vetor do incidente do `tj-actions/changed-files`, onde as tags de v1 a v45
   foram movidas para um commit malicioso e 23 mil repositórios foram atingidos.
   Como o subpath não tem tag utilizável, o Dependabot não consegue bumpar esse
   pin, então fica anotado para revisão manual trimestral
4. **`denied: permission_denied` na primeira release.** Não é o `permissions:` do
   workflow. O package novo nasce sem o repositório listado em Manage Actions
   access com role Write. Resolve na UI de settings do package
5. **`latest` apontando para release candidate.** O `flavor: latest=auto` do
   metadata-action marca latest em qualquer tag semver, incluindo `v0.3.0-rc.1`.
   Por isso `latest=false` mais um `type=raw` condicional ao prerelease
6. **Attestation no manifest por arquitetura em vez do índice.** Se o
   `attest-build-provenance` for chamado dentro do job de matrix, o digest que o
   usuário alcança pela tag fica sem attestation e a verificação falha. Só se
   descobre quando alguém reclama
7. **QEMU entrando pela porta dos fundos.** Se `docker/setup-qemu-action` for
   acrescentado, o buildx pode escolher o emulador em vez do runner arm64 nativo,
   e o build de quatro minutos vira noventa sem aviso
8. **Cache do buildx compartilhado entre arquiteturas.** Sem scope por
   arquitetura os dois jobs escrevem na mesma entrada e toda release começa fria
9. **Contact links apontando para categorias inexistentes.** Com
   `blank_issues_enabled: false`, se Q&A e Ideas não existirem em Discussions o
   único caminho de quem tem uma pergunta é um 404. As categorias vêm antes
10. **`Cargo.toml` esquecido na versão antiga.** Falha silenciosa: a imagem sai
    com `--version` mentindo. O job guard custa quinze segundos e elimina a
    classe inteira, mas depende do passo do `--version` ter sido feito
11. **A imagem do GHCR e o deploy do Fly divergem.** O `ci.yml` faz
    `flyctl deploy --remote-only`, que rebuilda o Dockerfile no Fly. Produção
    roda um binário que nunca passou pelo pipeline de release nem foi atestado.
    Trocar por `--image` fica para depois da v0.2.0, fora deste escopo
12. **PR único misturando configuração, workflow e código Rust.** Foi a escolha
    explícita do dono. O custo é que um problema em qualquer parte segura o
    resto, e a mitigação é que os commits dentro do PR são separados por fase,
    de modo que reverter uma fase isolada continue sendo possível

## Critério de aceite geral

- Insights > Community Standards marca todos os itens como presentes
- `gh repo view --json description,repositoryTopics,hasWikiEnabled` mostra
  descrição preenchida, vinte topics e wiki desligada
- `gh api repos/lucasolopes/quark --jq '.security_and_analysis'` mostra os
  quatro itens de secret scanning como enabled
- Todo `uses:` nos workflows está pinado por SHA de quarenta caracteres e todo
  workflow declara `permissions:` no topo
- `cargo deny check` passa local sem nenhum `--allow`
- Um PR de teste com CI vermelho mostra "Merging is blocked"
- `docker pull ghcr.io/lucasolopes/quark:0.2.0` funciona sem autenticação e
  `docker buildx imagetools inspect` lista `linux/amd64` e `linux/arm64`
- `gh attestation verify oci://ghcr.io/lucasolopes/quark:0.2.0 --repo lucasolopes/quark`
  responde verificação bem sucedida

## Referência

O relatório consolidado da pesquisa, com o conteúdo completo de cada arquivo e
os comandos prontos, é entregue junto em
`docs/research/2026-07-24-oss-readiness.md`.
