# Pesquisa: prontidão do repositório para open source profissional

Data: 2026-07-24. Consolidado de quatro frentes de pesquisa independentes
(community health, supply chain, release, descoberta) mais uma síntese.
Estado do repositório verificado via `gh api` na data acima.

Este documento é a referência de execução da spec em
`docs/superpowers/specs/2026-07-24-oss-repo-setup-design.md`. Ele traz o
conteúdo completo de cada arquivo a criar e os comandos prontos.

---

# quark: plano de execução para padrão open source profissional

Consolidado a partir de 4 frentes de pesquisa (health, security, release, discovery). Estado verificado do repo em 2026-07-24.

**Correções ao briefing que mudam o plano** (verificadas via API pela frente de security):

- **NÃO existe regra de commits assinados** no ruleset. As regras ativas são `deletion`, `non_fast_forward` e `pull_request`. Toda a preocupação "dependabot vs commits assinados" é vazia.
- **NÃO existe `required_status_checks`.** O CI não bloqueia merge. Esse é o buraco maior do que qualquer scanning que falte.
- **CodeQL já está ligado** (default setup: rust, javascript-typescript, actions, weekly).
- **Private Vulnerability Reporting já está ligado.**
- **Secret scanning e Dependabot alerts/security updates já ligados.** Falta só push protection.
- O dependency graph já ingere cargo (376 pacotes), npm (564) e actions (6).

---

## 1. VEREDITO

| # | Item | Decisão | Justificativa |
|---|---|---|---|
| 1 | Descrição do repo | **FAZER AGORA** | Campo vazio é o maior sinal de repo abandonado; custa um comando. |
| 2 | Topics (20) | **FAZER AGORA** | `quark` é nome colidido; topics carregam toda a descoberta. |
| 3 | Homepage | **FAZER DEPOIS** | Só quando existir Pages/landing. Apontar pra API JSON é pior que vazio. |
| 4 | Desligar Wiki | **FAZER AGORA** | Wiki vazia = porta pra lugar nenhum, e contribuição em wiki não passa por CLA nem review. |
| 5 | Push protection + non-provider patterns + validity checks | **FAZER AGORA** | Um comando; `QUARK_KEY` e `FLY_API_TOKEN` circulam no projeto. |
| 6 | `SECURITY.md` + PT_BR | **FAZER AGORA** | PVR já ligado mas sem política escrita ninguém acha o botão. |
| 7 | `CODE_OF_CONDUCT.md` + PT_BR (Covenant 2.1) | **FAZER AGORA** | Texto verbatim oficial, custo zero de manutenção, fecha Community Standards. |
| 8 | Issue forms (`bug.yml`, `feature.yml`, `config.yml`) | **FAZER AGORA** | Com 3 backends de store/cache/analytics plugáveis, form estruturado é o que salva sua triagem. |
| 9 | `question.yml` | **CORTAR** | Discussions já habilitado; vira `contact_link`. |
| 10 | `PULL_REQUEST_TEMPLATE.md` (~30 linhas) | **FAZER AGORA** | Só o que CI e bot de CLA não conseguem checar. |
| 11 | `CODEOWNERS` (2 linhas) | **FAZER AGORA** | Auto-request de review em PR externo. Não ligar "require code owner review". |
| 12 | `SUPPORT.md` | **CORTAR** | `config.yml` com contact_links aparece exatamente no momento em que a pessoa erraria de canal. Arquivo duplicado é manutenção. |
| 13 | `GOVERNANCE.md` | **CORTAR** | 7 stars, 1 fork, zero contribuidores externos. Governança escrita descrevendo processo nunca exercido lê como cargo cult. 6 linhas no CONTRIBUTING resolvem. |
| 14 | `CONTRIBUTING.md` reescrito + PT_BR | **FAZER AGORA** | Hoje não cita `web/`, i18n, regra bilíngue de docs, convenção de commit nem o gate de "require last push approval". |
| 15 | `.github/FUNDING.yml` | **FAZER DEPOIS** | Botão Sponsor só quando existir a landing de licença comercial ou Sponsors ativo. |
| 16 | Alias de e-mail para security/conduct | **CORTAR** | PVR + `github.com/contact/report-abuse` cobrem os dois casos sem criar caixa nova pra manter. |
| 17 | `.github/dependabot.yml` (cargo, npm, actions) | **FAZER AGORA** | Sem isso a árvore apodrece e Scorecard/Dependency-Update-Tool zera. |
| 18 | Dependabot para `docker` e `docker-compose` | **FAZER DEPOIS** | Só depois que o Dockerfile estiver pinado por digest (fase 3). |
| 19 | `permissions:` no topo dos workflows | **FAZER AGORA** | `ci.yml` não declara nada; o job `web` roda `npm ci` (564 pacotes, install scripts) com token de escrita. |
| 20 | Pinar actions por SHA | **FAZER AGORA** | `flyctl-actions@master` executa código arbitrário de terceiro no job que tem `FLY_API_TOKEN`. |
| 21 | `if: github.actor != 'dependabot[bot]'` no `cla.yml` | **FAZER AGORA** | Sem isso todo PR do bot nasce com X vermelho. Tem que ir **antes** do dependabot.yml. |
| 22 | `needs: [check, web]` no deploy do Fly | **FAZER AGORA** | Hoje deploya com o front quebrado. |
| 23 | `required_status_checks` no ruleset | **FAZER AGORA** | CI opcional é o buraco de segurança mais barato de fechar do repo. |
| 24 | `deny.toml` + workflow cargo-deny | **FAZER AGORA** | Único mecanismo que avisa se entrar dependência com licença incompatível com AGPL. |
| 25 | `cargo-audit` separado | **CORTAR** | cargo-deny engloba (mesma RustSec DB). Rodar os dois é ruído duplicado. |
| 26 | `dependency-review-action` | **FAZER AGORA** | 15 linhas, cobre npm e actions que o cargo-deny não vê. |
| 27 | Job `npm audit` | **CORTAR** | Dependabot alerts + dependency-review já cobrem; `npm audit` gera falso-positivo de devDependency. |
| 28 | `codeql.yml` avançado | **CORTAR** | Default setup já ativo; workflow avançado dá 409 no upload de SARIF. |
| 29 | Subir CodeQL para `security-extended` | **FAZER DEPOIS** | Um PATCH de API; faça depois que a fila de alertas estiver limpa. |
| 30 | `scorecard.yml` | **FAZER DEPOIS** | Rodar só depois das fases 2 e 3, senão a primeira nota pública é ~4.5. |
| 31 | Badge do Scorecard | **CORTAR até >7.5** | Badge 5.2 num projeto que vende segurança é pior que badge nenhum. |
| 32 | Reescrever `Dockerfile` (cargo-chef + pin + ca-certificates) | **FAZER AGORA (fase 3)** | Sem camada de deps o build de release é 10-15 min e o cache nunca esquenta. |
| 33 | Reescrever `.dockerignore` | **FAZER AGORA (fase 3)** | `node_modules/` da raiz entra no contexto e invalida a camada de build. |
| 34 | `release.yml` multiarch nativo (amd64 + arm64) | **FAZER AGORA (fase 3)** | Runner `ubuntu-24.04-arm` é grátis em repo público. Zero QEMU. |
| 35 | `CHANGELOG.md` + PT_BR manual (Keep a Changelog) | **FAZER AGORA (fase 3)** | Requisito bilíngue mata qualquer geração automática. |
| 36 | `release-please` / `git-cliff` em CI / `cargo-dist` | **CORTAR** | Geram changelog em pt-BR sem acento a partir dos commits, vazam IDs do Linear, e colidem com "require last push approval". |
| 37 | `docs/VERSIONING.md` + PT_BR | **CORTAR** | 10 linhas no cabeçalho do CHANGELOG dizem a mesma coisa e ninguém precisa manter dois arquivos. |
| 38 | `cliff.toml` | **CORTAR** | Rascunho local pode sair de `git log --oneline`. |
| 39 | `actions/attest-build-provenance` | **FAZER AGORA (fase 3)** | Uma linha, sem chave pra gerenciar, fecha Signed-Releases da Scorecard. |
| 40 | `cosign` | **CORTAR** | Mesmas garantias da attestation (é Sigstore por baixo), segundo formato pra documentar e quebrar. |
| 41 | Tag ruleset pra `v*` | **FAZER AGORA (fase 3)** | Ruleset de branch não cobre tags; hoje qualquer write publica `latest` no GHCR. |
| 42 | `--version` no binário + `version` no `/healthz` | **FAZER AGORA (fase 3)** | O guard do release e o campo de versão do bug form dependem disso. |
| 43 | Bump `Cargo.toml` para `0.2.0` + metadados de crate | **FAZER AGORA (fase 3)** | Fonte de verdade da versão; o guard do release compara com a tag. |
| 44 | Fly deployar a imagem publicada (`--image`) em vez de rebuildar | **FAZER DEPOIS** | Depois da v0.2.0. Hoje produção roda um binário que nunca passou pelo pipeline de release. |
| 45 | Embutir o painel admin na imagem | **FAZER DEPOIS (gate de 1.0)** | É mudança de código (rota `ServeDir` sob prefixo, conflito com `/:code`), não de pipeline. |
| 46 | README F1 (bloco de código órfão sem heading) | **FAZER AGORA (fase 1)** | É bug estrutural, corrige em 1 minuto. |
| 47 | README F2-F4 (quick start no topo, screenshot, tabela comparativa) | **FAZER AGORA (fase 4)** | Maior gap de conversão do arquivo. |
| 48 | README F5 (pt-BR vazando na tabela em inglês) | **FAZER AGORA (fase 4)** | Costura visível num arquivo polido. |
| 49 | Badges: trocar License por dinâmico, remover "Rust 2021" e um dos redundantes | **FAZER AGORA (fase 4)** | 4 de 5 badges são estáticos escritos à mão. |
| 50 | Badge de release + GHCR | **FAZER AGORA (fase 4, pós-tag)** | Só depois da v0.2.0; antes renderiza "no releases". |
| 51 | Social preview 1280x640 | **FAZER AGORA (fase 4)** | Maior ROI de asset único: é o card no HN, Reddit, Slack, X. Manual, não tem API. |
| 52 | awesome-selfhosted | **FAZER DEPOIS (~nov/2026)** | Regra dos 4 meses conta a partir do primeiro *release*. O relógio só começa na fase 3. |
| 53 | awesome-rust | **FAZER DEPOIS (50 stars)** | Gate é 50 stars ou 2k downloads no crates.io. |
| 54 | Show HN / r/selfhosted / selfh.st / AlternativeTo | **FAZER DEPOIS (pós-fase 4)** | Só faz sentido com imagem publicada, screenshot e social preview prontos. |
| 55 | GitHub Pages a partir de `docs/` | **FAZER DEPOIS** | Bom, mas é uma hora de trabalho que compete com a fase 3. |
| 56 | Fuzzing (cargo-fuzz em `permute.rs`/`codec.rs`) | **CORTAR (por ora)** | Tecnicamente natural, mas é projeto próprio, não item de higiene de repo. |
| 57 | CII Best Practices badge | **CORTAR** | Uma hora de formulário para um badge que ninguém no público-alvo lê. |
| 58 | `.github/DISCUSSION_TEMPLATE/`, MAINTAINERS, contributor ladder | **CORTAR** | Cerimônia pura com 1 pessoa. |

---

## 2. CONFLITOS ENTRE OS PESQUISADORES

**C1. Commits assinados no ruleset.** A frente de health escreveu uma seção inteira sobre documentar SSH signing no CONTRIBUTING, tratando isso como "a maior causa de PR abandonado". A frente de security verificou via API que **essa regra não existe**. Decisão: a frente de security ganha, o item sai do CONTRIBUTING. E não ligue a regra: com mantenedor solo, `required_signatures` só adiciona uma barreira ao primeiro contribuidor externo sem fechar nenhum vetor real (o merge já passa por PR + 1 aprovação).

**C2. `SECURITY.md` com e-mail de fallback.** Health propôs criar um alias descartável (`quark-security@...`). Security apontou que PVR já está ligado. Decisão: **sem e-mail nenhum**. O SECURITY.md aponta só para o formulário de advisory, e o CoC aponta para `github.com/contact/report-abuse` como escape hatch. Zero caixa nova para manter, zero endereço pessoal exposto.

**C3. `SUPPORT.md`.** Health defendeu ("custa 15 linhas, economiza triagem indefinidamente"), citando que nenhum dos 5 pares tem. Decisão: **cortar**. O argumento dele contra ele mesmo: nenhum par tem. E o `config.yml` com `blank_issues_enabled: false` intercepta a pessoa exatamente no clique errado, o que um arquivo na sidebar não faz.

**C4. `docs/VERSIONING.md`.** Release propôs arquivo + gêmeo. Decisão: **cortar o arquivo**, colocar o parágrafo de semântica 0.x no cabeçalho do `CHANGELOG.md`. Dois arquivos a mais para explicar uma convenção de três linhas é exatamente o tipo de doc que envelhece sozinho.

**C5. Primeira tag: `v0.1.0` ou `v0.2.0`.** Discovery quer tag "esta semana" para iniciar o relógio de 4 meses do awesome-selfhosted, sem opinar sobre o número. Release argumenta `v0.2.0` porque a `0.1.0` do `Cargo.toml` nunca saiu e o software já tem OIDC, multi-tenant, webhooks e ClickHouse. Decisão: **`v0.2.0`**. As duas satisfazem o relógio; a diferença é que `v0.1.0` deixaria a impressão de protótipo num software que claramente não é.

**C6. Badge do OpenSSF Scorecard.** Security recomenda rodar o workflow agora e segurar o badge até 7.5. Discovery diz o mesmo com outras palavras. Sem conflito real, mas eu vou mais longe: **o workflow também espera** até depois da fase 3, senão o primeiro resultado público (o dashboard é público mesmo sem badge) registra ~4.5.

**C7. `security-extended` no CodeQL.** Security recomenda subir. Decisão: **depois**. Falso-positivo de query estendida em código Rust ainda é irregular e você não tem fila de triagem sobrando. Suba quando a aba Security estiver zerada com o suite default.

**C8. Homepage.** Discovery ranqueou Pages > painel > vazio, com "opção interina" de apontar pra pasta docs. Decisão: **deixar vazio** até existir Pages. Apontar homepage pra `github.com/.../tree/main/docs` é um link circular que sinaliza improviso.

**C9. Escopo do `dependabot.yml`.** Security propôs 5 ecossistemas (cargo, npm, actions, docker, docker-compose). Decisão: **3 agora**. Os blocos docker só fazem sentido depois que o `Dockerfile` estiver pinado por digest (fase 3), senão o Dependabot não tem o que atualizar.

---

## 3. PLANO EM FASES

### Fase 1: metadados + health files
Sem código. Entrega: repo deixa de parecer abandonado e a triagem fica estruturada.

| # | Tarefa | Arquivo / comando | Critério de aceite |
|---|---|---|---|
| 1.1 | Descrição, topics, wiki off | bloco de comandos da seção 5 (linhas 1-3) | `gh repo view lucasolopes/quark --json description,repositoryTopics,hasWikiEnabled` mostra descrição preenchida, 20 topics, wiki `false` |
| 1.2 | Criar categorias Q&A e Ideas em Discussions | UI: Settings > Discussions, ou aba Discussions > "New category" | As URLs `/discussions/categories/q-a` e `/discussions/categories/ideas` respondem 200 (senão o `config.yml` vira 404) |
| 1.3 | Push protection + patterns + validity checks | bloco da seção 5 (linha 4) | `gh api repos/lucasolopes/quark --jq '.security_and_analysis'` mostra os 4 como `enabled` |
| 1.4 | Corrigir o bloco órfão do README (F1) | `README.md` linha ~179, adicionar `## Quick start` antes do bloco ```bash; espelhar em `README.PT_BR.md` | Nenhum bloco de código sem heading acima no arquivo |
| 1.5 | PR 1: política de segurança e conduta | criar `SECURITY.md`, `SECURITY.PT_BR.md`, `CODE_OF_CONDUCT.md`, `CODE_OF_CONDUCT.PT_BR.md` | Aba Insights > Community Standards marca "Security policy" e "Code of conduct" como presentes |
| 1.6 | PR 2: templates e CODEOWNERS | criar `.github/ISSUE_TEMPLATE/bug.yml`, `feature.yml`, `config.yml`, `.github/PULL_REQUEST_TEMPLATE.md`, `.github/CODEOWNERS` | Clicar em "New issue" mostra os 2 forms + 4 contact links e **não** mostra "Open a blank issue"; abrir um PR de teste preenche o template e auto-requisita review |
| 1.7 | PR 3: CONTRIBUTING reescrito | reescrever `CONTRIBUTING.md` e `CONTRIBUTING.PT_BR.md`; adicionar linha de CoC no `README.md` e `README.PT_BR.md` | Os dois arquivos cobrem: frontend, i18n, regra bilíngue de docs, convenção de commit, "require last push approval", o que não será mergeado, quem decide |

### Fase 2: automação (dependabot + supply chain)
Entrega: dependências atualizadas sozinhas e nada entra sem passar por licença e advisory.

| # | Tarefa | Arquivo / comando | Critério de aceite |
|---|---|---|---|
| 2.1 | Blindar o `cla.yml` contra o Dependabot | `.github/workflows/cla.yml`: adicionar `if: github.actor != 'dependabot[bot]'` no job | Um PR do bot não dispara o job de CLA (aparece como skipped, não failed) |
| 2.2 | Hardening dos workflows | reescrever `.github/workflows/ci.yml` (seção 4), pinar as 2 actions do `cla.yml` por SHA | `grep -c 'uses:.*@[0-9a-f]\{40\}'` cobre 100% dos `uses:`; nenhum `@master`; todo workflow tem `permissions:` no topo |
| 2.3 | Criar as labels que faltam | bloco da seção 5 (linha 5) | `gh label list` inclui `github-actions` |
| 2.4 | Ativar version updates | criar `.github/dependabot.yml` (seção 4) | Em até 24h o bot abre PRs agrupados; nenhum deles com X vermelho de CLA |
| 2.5 | cargo-deny | rodar `~/.cargo/bin/cargo.exe install cargo-deny --locked` e `cargo deny check --all-features` local, preencher `[bans].skip` com as duplicatas reais, commitar `deny.toml` | `cargo deny check` passa local sem `--allow` nenhum |
| 2.6 | Workflows de supply chain | criar `.github/workflows/supply-chain.yml` e `.github/workflows/dependency-review.yml` | Os 4 jobs aparecem verdes num PR de teste |
| 2.7 | Tornar o CI obrigatório | bloco da seção 5 (linha 6-7): baixar o ruleset, acrescentar `required_status_checks`, dar PUT | `gh api repos/lucasolopes/quark/rulesets/19673028 --jq '.rules[].type'` inclui `required_status_checks`; um PR com CI vermelho mostra "Merging is blocked" |

### Fase 3: release Docker GHCR
Entrega: `docker run ghcr.io/lucasolopes/quark:0.2` funciona para qualquer pessoa, em amd64 e arm64, com provenance verificável.

| # | Tarefa | Arquivo / comando | Critério de aceite |
|---|---|---|---|
| 3.1 | `--version` no binário e `version` no `/healthz` | `src/main.rs` (parse de arg antes do runtime), `src/api/router.rs` | `cargo run -- --version` imprime `quark 0.2.0`; `curl localhost:8080/healthz` traz o campo `version` |
| 3.2 | Bump e metadados de crate | `Cargo.toml`: `version = "0.2.0"`, `repository`, `homepage`, `readme`, `keywords`, `categories` | `cargo metadata --locked` passa e a versão bate com a tag que será criada |
| 3.3 | Dockerfile e .dockerignore | reescrever ambos (seção 4) | `docker build .` local em <4 min na segunda vez; `docker run --rm <img> quark --version` responde |
| 3.4 | CHANGELOG | criar `CHANGELOG.md` e `CHANGELOG.PT_BR.md` com a seção `## [0.2.0]` | O heading é exatamente `## [0.2.0] - AAAA-MM-DD` (o `awk` do release depende disso) |
| 3.5 | Workflow de release | criar `.github/workflows/release.yml` (seção 4), pinando as actions por SHA | Merge na main sem disparar nada (só `on: push: tags`) |
| 3.6 | Tag ruleset | bloco da seção 5 (linha 8) | Ninguém além do dono cria tag `v*` |
| 3.7 | Primeira release | `git tag -a v0.2.0 -m "quark v0.2.0" && git push origin v0.2.0` | Os 4 jobs verdes; `docker buildx imagetools inspect ghcr.io/lucasolopes/quark:0.2.0` lista `linux/amd64` e `linux/arm64` |
| 3.8 | Tornar o package público | UI: `github.com/users/lucasolopes/packages/container/quark/settings` > Change visibility > Public; e conferir **Manage Actions access** com o repo em role Write | `docker logout ghcr.io && docker pull ghcr.io/lucasolopes/quark:0.2.0` funciona anônimo |
| 3.9 | Verificar provenance | `gh attestation verify oci://ghcr.io/lucasolopes/quark:0.2.0 --repo lucasolopes/quark` | Saída "Verification succeeded" |

### Fase 4: descoberta
Entrega: quem chega no repo entende o produto em 20 segundos.

| # | Tarefa | Arquivo / comando | Critério de aceite |
|---|---|---|---|
| 4.1 | Screenshot do painel | PNG 1200px da lista de links em `docs/assets/panel.png`, referenciado logo abaixo dos quick links | README mostra evidência visual do painel antes da linha 20 |
| 4.2 | Reordenar o README (F2) | mover o `## Quick start` (com o `docker run` real da fase 3) para logo depois dos quick links; idem no PT_BR | Um leitor novo encontra o comando de rodar sem rolar a página |
| 4.3 | Tabela comparativa (F4) | nova seção depois do quick start: quark / Shlink / YOURLS / Kutt / Dub x Linguagem, Serviços exigidos, Tamanho, Painel, Licença | A coluna "serviços exigidos: nenhum" fica visível |
| 4.4 | Anglicizar a tabela de avalanche (F5) | `README.md` linhas 82-91: `avg_avalanche`, `coverage(/40)`, `← ROUNDS chosen (diffusion closes)` | Zero português no README em inglês |
| 4.5 | Especificar a máquina dos benchmarks (F6) | uma linha com CPU, cores, RAM, OS, versão do rustc acima das tabelas | Nenhuma tabela diz "measured on this machine" sem dizer qual |
| 4.6 | Badges | remover "Rust 2021" e um dos dois redundantes; License vira `img.shields.io/github/license/lucasolopes/quark`; adicionar release (`sort=semver`) e GHCR | Máximo 5 badges, nenhum estático que possa divergir do repo |
| 4.7 | Enxugar a tabela de config inline | reduzir de 17 para 6 variáveis, linkar `docs/CONFIGURATION.md`; agrupar a seção "More" em Deploy / Features / Reference | README abaixo de 280 linhas |
| 4.8 | Social preview | PNG 1280x640, conteúdo dentro de 1120x520: wordmark **quark**, tagline "short codes are computed, not stored", chips `Rust` · `single binary ~1 MB` · `zero runtime deps`. Upload em Settings > General > Social preview | Colar a URL do repo num DM de Slack renderiza o card |
| 4.9 | Homepage | só quando Pages existir: `gh repo edit lucasolopes/quark --homepage https://lucasolopes.github.io/quark` | Link na sidebar leva a docs, não a JSON |
| 4.10 | Divulgação | Show HN + r/selfhosted + r/rust, depois selfh.st e AlternativeTo | Feito só depois de 4.1-4.8 |
| 4.11 | Scorecard | criar `.github/workflows/scorecard.yml`, rodar, ler a nota | Nota ≥ 7.5 antes de colar o badge |

---

## 4. ARQUIVOS

### 4.1 `SECURITY.md`

```markdown
**English** · [Português](SECURITY.PT_BR.md)

# Security Policy

## Reporting a vulnerability

Do not open a public issue, discussion, or pull request for a security problem.

Report it privately through GitHub:
**https://github.com/lucasolopes/quark/security/advisories/new**

That form is private, creates a draft advisory, and lets us coordinate a fix and
a CVE in the same place. There is no security email address and no PGP key: the
advisory form is the only channel.

Please include, as far as you can:

- the version: release tag, GHCR image digest, or commit SHA (`quark --version`)
- the deployment shape: single binary or Docker, store backend (LMDB or
  Postgres), cache (in-process or Valkey), analytics sink (embedded or ClickHouse)
- reproduction steps or a proof of concept, ideally as `curl` calls
- the impact you believe it has

## What to expect

quark is maintained by one person in their own time. These are realistic
targets, not a contractual SLA.

| Step | Target |
| --- | --- |
| First human reply | 5 business days |
| Triage decision (accepted, not a vulnerability, or needs more info) | 10 business days |
| Fix released for accepted high or critical reports | 30 days after triage |
| Public advisory | with the fix, or 90 days after the report, whichever comes first |

If you get no reply within 10 business days, open a public issue titled
"security report awaiting response" with **no technical details** and we will
pick the thread back up.

We follow coordinated disclosure. Please give us 90 days before publishing.
There is no bug bounty. Accepted reports get credit in the advisory unless you
ask otherwise.

## Supported versions

quark is pre-1.0. There are no maintenance branches and nothing is backported.
Fixes land on `main` and ship in the next `ghcr.io/lucasolopes/quark` image.

| Version | Supported |
| --- | --- |
| `main` and the latest GHCR image tag | yes |
| any earlier tag or image | no, upgrade |

## Scope

In scope, roughly ordered by how much we care:

- short code predictability or enumeration: anything that recovers the internal
  id or the key material from codes, or that lowers the measured avalanche below
  the calibrated threshold
- admin authentication and authorization bypass: `src/api/guard.rs`, API tokens
  and scopes in `src/auth.rs`, OIDC login and SSO domain mapping
- tenant isolation breaks: reading or writing another tenant's links, domains,
  or analytics
- SSRF and open redirect bypasses in `src/abuse/` (`is_internal_host`,
  `extract_host`) and in link creation
- password protected link bypass, expired or disabled link still resolving
- webhook signature forgery or replay (Standard Webhooks implementation)
- XSS, CSRF, or session handling flaws in the admin panel under `web/`
- secrets leaking into logs, analytics events, or API responses
  (`QUARK_KEY`, `QUARK_ADMIN_TOKEN`, OIDC client secrets, webhook secrets)
- rate limit bypass that turns into a practical denial of service

Out of scope:

- missing hardening headers, cookie flags, or TLS configuration with no
  demonstrated exploit
- self-XSS, clickjacking on unauthenticated pages, or attacks needing physical
  or already-root access to the host
- volumetric denial of service against a demo or third-party instance
- automated scanner output with no working proof of concept
- operator misconfiguration: reusing `QUARK_KEY` across deployments, shipping a
  default `QUARK_ADMIN_TOKEN`, exposing the admin API to the internet without a
  proxy. Those are documented in `docs/CONFIGURATION.md`, not vulnerabilities.
- vulnerabilities in Postgres, Valkey, ClickHouse, or other dependencies without
  a quark-specific exploit path. Report those upstream.

One note on `QUARK_KEY`: it is the secret behind the code permutation. Anyone
who has it can enumerate every code on that instance. Treat its exposure as a
compromise of the whole link namespace and rotate it, which invalidates existing
codes.
```

### 4.2 `SECURITY.PT_BR.md`

```markdown
[English](SECURITY.md) · **Português**

# Política de segurança

## Como reportar uma vulnerabilidade

Não abra issue, discussion ou pull request público para um problema de
segurança.

Reporte de forma privada pelo GitHub:
**https://github.com/lucasolopes/quark/security/advisories/new**

Esse formulário é privado, cria um advisory em rascunho e permite coordenar a
correção e o CVE no mesmo lugar. Não existe e-mail de segurança nem chave PGP: o
formulário de advisory é o único canal.

Inclua, na medida do possível:

- a versão: tag de release, digest da imagem do GHCR ou SHA do commit
  (`quark --version`)
- o formato do deploy: binário único ou Docker, backend de store (LMDB ou
  Postgres), cache (em processo ou Valkey), destino de analytics (embutido ou
  ClickHouse)
- passos de reprodução ou uma prova de conceito, de preferência com `curl`
- o impacto que você acredita que existe

## O que esperar

O quark é mantido por uma pessoa só, no tempo livre dela. Os prazos abaixo são
metas realistas, não um SLA contratual.

| Etapa | Meta |
| --- | --- |
| Primeira resposta humana | 5 dias úteis |
| Decisão de triagem (aceito, não é vulnerabilidade, ou falta informação) | 10 dias úteis |
| Correção publicada para relatos aceitos de severidade alta ou crítica | 30 dias após a triagem |
| Advisory público | junto com a correção, ou 90 dias após o relato, o que vier primeiro |

Se você não receber resposta em 10 dias úteis, abra uma issue pública com o
título "security report awaiting response" e **nenhum detalhe técnico**, que a
gente retoma a conversa.

Seguimos divulgação coordenada. Aguarde 90 dias antes de publicar. Não há
programa de recompensa. Relatos aceitos recebem crédito no advisory, a não ser
que você prefira o contrário.

## Versões suportadas

O quark é pré-1.0. Não existem branches de manutenção e nada é backportado.
Correções entram na `main` e saem na próxima imagem
`ghcr.io/lucasolopes/quark`.

| Versão | Suportada |
| --- | --- |
| `main` e a tag mais recente da imagem no GHCR | sim |
| qualquer tag ou imagem anterior | não, atualize |

## Escopo

Dentro do escopo, mais ou menos em ordem de prioridade:

- previsibilidade ou enumeração de códigos curtos: qualquer coisa que recupere o
  id interno ou o material da chave a partir dos códigos, ou que derrube o
  avalanche medido abaixo do limite calibrado
- bypass de autenticação e autorização do admin: `src/api/guard.rs`, tokens de
  API e escopos em `src/auth.rs`, login OIDC e mapeamento de domínios SSO
- quebra de isolamento entre tenants: ler ou escrever links, domínios ou
  analytics de outro tenant
- bypass de SSRF e open redirect em `src/abuse/` (`is_internal_host`,
  `extract_host`) e na criação de links
- bypass de link protegido por senha, link expirado ou desativado que ainda
  resolve
- forja ou replay de assinatura de webhook (implementação Standard Webhooks)
- XSS, CSRF ou falhas de sessão no painel admin em `web/`
- vazamento de segredos em logs, eventos de analytics ou respostas da API
  (`QUARK_KEY`, `QUARK_ADMIN_TOKEN`, client secrets de OIDC, segredos de
  webhook)
- bypass de rate limit que vire negação de serviço na prática

Fora do escopo:

- ausência de headers de hardening, flags de cookie ou configuração de TLS sem
  exploração demonstrada
- self-XSS, clickjacking em páginas não autenticadas, ou ataques que exijam
  acesso físico ou já privilegiado à máquina
- negação de serviço volumétrica contra uma demo ou instância de terceiros
- saída de scanner automático sem prova de conceito funcionando
- erro de configuração do operador: reusar `QUARK_KEY` entre deploys, subir com
  `QUARK_ADMIN_TOKEN` padrão, expor a API admin na internet sem proxy. Isso está
  documentado em `docs/CONFIGURATION.PT_BR.md`, não é vulnerabilidade.
- vulnerabilidades em Postgres, Valkey, ClickHouse ou outras dependências sem um
  caminho de exploração específico do quark. Reporte no projeto de origem.

Uma observação sobre a `QUARK_KEY`: ela é o segredo por trás da permutação dos
códigos. Quem tiver a chave consegue enumerar todos os códigos daquela
instância. Trate o vazamento dela como comprometimento de todo o namespace de
links e rotacione, o que invalida os códigos existentes.
```

### 4.3 `CODE_OF_CONDUCT.md` e `CODE_OF_CONDUCT.PT_BR.md`

Não reescreva o Contributor Covenant à mão nem deixe um modelo parafrasear: use o texto oficial verbatim. Busque os dois arquivos e aplique **uma** edição em cada.

```bash
curl -fsSL https://raw.githubusercontent.com/EthicalSource/contributor_covenant/release/content/version/2/1/code_of_conduct.md      -o CODE_OF_CONDUCT.md
curl -fsSL https://raw.githubusercontent.com/EthicalSource/contributor_covenant/release/content/version/2/1/code_of_conduct.pt-br.md -o CODE_OF_CONDUCT.PT_BR.md
# remova o front matter YAML do topo de cada arquivo (o bloco entre --- e ---)
```

No `CODE_OF_CONDUCT.md`, no parágrafo de Enforcement, substitua a frase com `[INSERT CONTACT METHOD]` por:

```markdown
Instances of abusive, harassing, or otherwise unacceptable behavior may be
reported to the maintainer by opening a
[private report to the repository](https://github.com/lucasolopes/quark/security/advisories/new)
or by contacting @lucasolopes directly on GitHub. All complaints will be
reviewed and investigated promptly and fairly.

quark has a single maintainer, so there is no separate enforcement committee. If
your report is about the maintainer, or you would rather not write to them at
all, use GitHub's own reporting instead:
https://github.com/contact/report-abuse
```

E na primeira linha de cada arquivo, o header de troca de idioma:
`**English** · [Português](CODE_OF_CONDUCT.PT_BR.md)` e `[English](CODE_OF_CONDUCT.md) · **Português**`.

No `CODE_OF_CONDUCT.PT_BR.md`, o parágrafo equivalente:

```markdown
Casos de comportamento abusivo, de assédio ou inaceitável de qualquer outra
forma podem ser reportados ao mantenedor abrindo um
[relato privado no repositório](https://github.com/lucasolopes/quark/security/advisories/new)
ou falando com @lucasolopes diretamente no GitHub. Todas as reclamações serão
analisadas e investigadas de forma rápida e justa.

O quark tem um mantenedor só, então não existe comitê de aplicação separado. Se
o seu relato for sobre o mantenedor, ou se você preferir não falar com ele,
use o canal do próprio GitHub: https://github.com/contact/report-abuse
```

### 4.4 `.github/ISSUE_TEMPLATE/bug.yml`

```yaml
name: Bug report
description: Something in quark is broken or does not match the docs
labels: ["bug"]
body:
  - type: markdown
    attributes:
      value: |
        Security problems do not go here. Report them privately at
        https://github.com/lucasolopes/quark/security/advisories/new

        Pode responder em português, se preferir.

  - type: checkboxes
    id: preflight
    attributes:
      label: Before you file
      options:
        - label: I searched open and closed issues and this is not a duplicate
          required: true
        - label: This is not a security vulnerability
          required: true

  - type: input
    id: version
    attributes:
      label: quark version
      description: Output of `quark --version`, the GHCR image digest, or a commit SHA
      placeholder: "quark 0.2.0"
    validations:
      required: true

  - type: dropdown
    id: deployment
    attributes:
      label: How are you running quark
      options:
        - Single binary from source (cargo run / cargo build --release)
        - Docker image from GHCR
        - docker compose (repo compose file)
        - Fly.io
        - Kubernetes
        - Other (describe in steps)
    validations:
      required: true

  - type: dropdown
    id: store
    attributes:
      label: Store backend
      options: ["LMDB (embedded default)", "Postgres"]
    validations:
      required: true

  - type: dropdown
    id: cache
    attributes:
      label: Cache
      options: ["In-process only (default)", "Valkey or Redis L2"]
    validations:
      required: true

  - type: dropdown
    id: analytics
    attributes:
      label: Analytics sink
      options: ["Embedded (default)", "ClickHouse", "Not used"]
    validations:
      required: true

  - type: dropdown
    id: area
    attributes:
      label: Which area
      options:
        - Redirect hot path (/:code)
        - Admin API (link CRUD, tenants, domains, invites)
        - Admin panel (web/)
        - Auth (admin token, API tokens and scopes, OIDC, SSO domains)
        - Analytics and aggregates
        - Webhooks
        - Import, sheets, pixels
        - Docs
        - Something else
    validations:
      required: true

  - type: textarea
    id: current
    attributes:
      label: What happens
    validations:
      required: true

  - type: textarea
    id: expected
    attributes:
      label: What you expected
    validations:
      required: true

  - type: textarea
    id: steps
    attributes:
      label: Steps to reproduce
      description: >
        Smallest reproduction that still fails. `curl` calls against a fresh
        instance are ideal. A repro that needs a cloud provider, a cluster, or
        your private reverse proxy is usually not reproducible for us and the
        issue will be closed as such.
      placeholder: |
        1. QUARK_KEY=... cargo run --release
        2. curl -X POST localhost:8080/api/links -d '{"url":"..."}'
        3.
    validations:
      required: true

  - type: textarea
    id: logs
    attributes:
      label: Logs
      description: >
        Run with `RUST_LOG=debug`. Redact QUARK_KEY, QUARK_ADMIN_TOKEN, API
        tokens, OIDC client secrets, and webhook secrets before pasting.
      render: text

  - type: input
    id: browser
    attributes:
      label: Browser and OS
      description: Only needed for admin panel bugs
      placeholder: "Firefox 141 on Ubuntu 24.04"
```

### 4.5 `.github/ISSUE_TEMPLATE/feature.yml`

```yaml
name: Feature request
description: Suggest a capability or a change in behavior
labels: ["enhancement"]
body:
  - type: markdown
    attributes:
      value: |
        Pode responder em português, se preferir.

  - type: checkboxes
    id: preflight
    attributes:
      label: Before you file
      options:
        - label: I searched open and closed issues and this is not a duplicate
          required: true
        - label: I checked docs/ROADMAP.md and this is not already planned
          required: true

  - type: textarea
    id: problem
    attributes:
      label: The problem
      description: >
        Describe the problem, not the solution. What are you trying to do, and
        what makes it hard or impossible today?
    validations:
      required: true

  - type: textarea
    id: proposal
    attributes:
      label: What you have in mind
      description: >
        Optional. If it adds configuration, say which `QUARK_*` variable and
        what its default would be. If it adds a dependency, say which and why.

  - type: textarea
    id: alternatives
    attributes:
      label: Workarounds you tried
      description: How do you handle this today, outside quark or with a hack?

  - type: dropdown
    id: area
    attributes:
      label: Which area
      options:
        - Redirect hot path (/:code)
        - Admin API
        - Admin panel (web/)
        - Auth and multi-tenancy
        - Analytics
        - Webhooks and integrations
        - Deployment, config, or docs
        - Something else
    validations:
      required: true

  - type: dropdown
    id: contribution
    attributes:
      label: Would you be up for implementing this
      description: >
        No pressure. It just helps a solo maintainer decide what to pick up
        versus what to leave open for a contributor.
      options:
        - "Yes, if the direction is agreed first"
        - "Maybe, with guidance"
        - "No, just requesting"
    validations:
      required: true
```

### 4.6 `.github/ISSUE_TEMPLATE/config.yml`

```yaml
blank_issues_enabled: false
contact_links:
  - name: Question or help getting quark running
    url: https://github.com/lucasolopes/quark/discussions/categories/q-a
    about: Setup, configuration, and usage questions belong in Discussions, not the issue tracker.
  - name: Idea or feedback
    url: https://github.com/lucasolopes/quark/discussions/categories/ideas
    about: Half-formed ideas and "would you consider" questions. Concrete requests can go straight to a feature request.
  - name: Report a security vulnerability
    url: https://github.com/lucasolopes/quark/security/advisories/new
    about: Private disclosure. Never open a public issue for a security problem.
  - name: Documentation
    url: https://github.com/lucasolopes/quark/tree/main/docs
    about: API, configuration, architecture, deploy, and scaling docs, in English and Portuguese.
```

### 4.7 `.github/PULL_REQUEST_TEMPLATE.md`

```markdown
## What and why

<!--
What changes, and what problem it solves. If there is an issue, link it:
Closes #123
For anything larger than a fix, please open an issue first so we can agree on
direction before you write the code.
-->

## How it was verified

<!--
Beyond CI. What did you actually run or click? For panel changes, add a
before/after screenshot.
-->

## Checklist

<!-- CI already runs fmt, clippy -D warnings, cargo test, cargo-deny, and the
web lint/typecheck/test/build job. The CLA bot handles the CLA. These are the
things automation cannot check. -->

- [ ] Behavior changes are covered by tests (`tests/*_it.rs` for API surface,
      inline `#[cfg(test)]` for units, Vitest for `web/`)
- [ ] Docs updated, **including the `.PT_BR.md` twin**, if behavior, config, or
      the API changed
- [ ] New `QUARK_*` variables documented in `docs/CONFIGURATION.md` and its twin
- [ ] New panel strings added to **both** `web/src/i18n/en.ts` and
      `web/src/i18n/pt-BR.ts`
- [ ] `CHANGELOG.md` updated under `## [Unreleased]`, or not applicable
- [ ] No new runtime dependency, or the PR explains why it is worth the binary
      size and the "no runtime deps" promise
- [ ] The redirect hot path (`/:code`) stays allocation-light; if it was
      touched, `cargo bench` numbers are in the description

## Breaking changes and migration

<!-- Config, API, storage layout, or panel behavior that an operator has to act
on when upgrading. Write "none" if there are none. -->
```

### 4.8 `.github/CODEOWNERS`

```
# Every change is reviewed by the maintainer. One line today; split by area
# when there is a second maintainer.
#
# Do NOT turn on "Require review from Code Owners" in the ruleset while the
# project has a single maintainer: combined with require_last_push_approval and
# the fact that you cannot approve your own PR, it makes every maintainer PR
# depend on the admin bypass.
*  @lucasolopes
```

### 4.9 `CONTRIBUTING.md` (reescrito)

```markdown
**English** · [Português](CONTRIBUTING.PT_BR.md)

# Contributing to quark

Thanks for your interest. quark is open source under the **GNU AGPLv3** (see
[`LICENSE`](LICENSE)). Contributions of code, docs, tests, and bug reports are
welcome.

By taking part you agree to the [Code of Conduct](CODE_OF_CONDUCT.md).

## Contributor License Agreement (required)

Before your pull request can be merged, you must accept the
[Contributor License Agreement](CLA.md). It is a **license grant, not a copyright
transfer**: **you keep full ownership of your contributions**. You grant the
maintainer a broad license (including the right to relicense) so quark can be
offered both under the AGPL and, separately, under a commercial license and a
hosted edition. Same model as Dub, n8n and Grafana.

Signing is a **one-time click**: when you open your first PR a bot posts a link;
accept it once and it covers every future PR.

## Ways to contribute

- **Questions and setup help** go to
  [Discussions, Q&A](https://github.com/lucasolopes/quark/discussions/categories/q-a),
  not the issue tracker.
- **Bugs** go to the [bug form](https://github.com/lucasolopes/quark/issues/new?template=bug.yml),
  with a reproduction against a fresh instance.
- **Security problems** never go in public. See [SECURITY.md](SECURITY.md).
- **Picking up work**: issues labeled `good first issue` and `help wanted` are
  free to take. Comment on the issue first so two people do not write the same
  patch.

## Development

Prerequisites: a stable Rust toolchain (via [rustup](https://rustup.rs)) and
Node 20+ for the admin panel. Depth lives in
[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md); this is only what you need for a
first PR.

Backend:

```bash
cargo build
cargo test          # lib + API tests, no external services needed
cargo fmt --all
cargo clippy --all-targets -- -D warnings
```

Admin panel (`web/`, React + TypeScript + Vite):

```bash
cd web
npm ci
npm run dev
npm run lint        # oxlint, --max-warnings 0
npm run typecheck   # tsc -b, this one has broken a deploy before
npm run test        # Vitest
npm run build
```

Integration tests for Postgres, Valkey and ClickHouse are gated behind
`QUARK_TEST_DATABASE_URL`, `QUARK_TEST_VALKEY_URL` and
`QUARK_TEST_CLICKHOUSE_URL`, and are skipped when unset. Most changes do not
need them.

## Tests

- API surface: integration tests in `tests/*_it.rs`. Build the `AppState`
  through the shared `TestState` builder in `tests/common/mod.rs`, not a
  hand-rolled struct literal.
- Units: inline `#[cfg(test)]` modules next to the code.
- Panel: `web/src/**/*.test.tsx` with Vitest.
- Keep the **redirect hot path** allocation-light. It is the performance
  critical path, see [`benches/redirect_bench.rs`](benches/redirect_bench.rs).

## Docs and i18n rules

Two rules that are easy to miss and that we will ask for in review:

1. **Every user-facing doc has a `.PT_BR.md` twin.** `docs/WEBHOOKS.md` and
   `docs/WEBHOOKS.PT_BR.md`. Both start with the language switch header:
   `**English** · [Português](X.PT_BR.md)` and the mirror on the twin. A doc PR
   in one language only is incomplete.
2. **Every new panel string goes in both `web/src/i18n/en.ts` and
   `web/src/i18n/pt-BR.ts`.** No hardcoded strings in components.

Prose style: plain direct technical English, no em dashes, natural pt-BR on the
twin. Do not translate literally, write it as the language would.

## New dependencies

"Zero runtime dependencies" and "~1 MB binary" are the project's pitch. A new
crate needs a justification in the PR description: what it does, why the std
library or an existing dependency cannot, and what it costs in binary size. For
`web/` the bar is higher still, since the bundle ships to every panel user.

## Commits, branches, and pull requests

- Branches: `feat/short-slug`, `fix/short-slug`, `chore/short-slug`.
- Commits: [Conventional Commits](https://www.conventionalcommits.org/) with a
  scope, `feat(web):`, `fix(api):`, `docs:`, `chore:`. **Write commit messages
  in English.** Older history is in Portuguese; it stays as is.
- Update `CHANGELOG.md` under `## [Unreleased]` in the same PR, not later.
- Fork the repo and open the PR against `main`.

What the merge gate looks like, so nothing surprises you:

- CI must pass: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`,
  cargo-deny, dependency review, and the web `lint`/`typecheck`/`test`/`build`
  job.
- One approving review is required, and every review thread must be resolved.
- **Any push after an approval dismisses that approval.** The branch rule is
  "require last push approval", so a last-minute typo fix means asking for
  review again. Batch your changes.
- Expect a first response within a week. Silence means "not looked at yet".

## What we will not merge

Saving you the work up front:

- changes to the redirect hot path that trade latency for convenience
- a new store backend without someone committed to maintaining it
- a dependency on an external service in the default path
- mass style rewrites, reformatting, or renames unrelated to a fix
- changes to the short code scheme. It is the core of the project and needs a
  design spec in `docs/specs/` agreed before any code.

## Who decides

quark has a single maintainer, @lucasolopes, who has the final say on scope,
design, and what gets merged. The direction is public in
[docs/ROADMAP.md](docs/ROADMAP.md), and design specs land in `docs/specs/`
before the code does, so you can argue with a decision while it is still cheap
to change.

The [CLA](CLA.md) lets the project be offered under the AGPL and, separately,
under a commercial license. That is a deliberate choice, not a step toward
closing the source: the AGPL edition is the project, not a teaser. If a
governance model with more than one maintainer ever makes sense, it will be
written down here first.

## Where things are

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md): how the pieces fit together.
- [`docs/DEVELOPMENT.md`](docs/DEVELOPMENT.md): the full development guide.
- [`docs/ROADMAP.md`](docs/ROADMAP.md): direction and what is next.
- [`docs/SCALING.md`](docs/SCALING.md): deployment shapes and their limits.
```

### 4.10 `CONTRIBUTING.PT_BR.md`

```markdown
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
- Atualize o `CHANGELOG.md` em `## [Unreleased]` no mesmo PR, não depois.
- Faça fork e abra o PR contra a `main`.

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
```

### 4.11 `.github/dependabot.yml`

```yaml
# Version updates. Security updates are separate (already enabled on the repo)
# and are NOT subject to open-pull-requests-limit or to cooldown.
#
# Grouping is aggressive on purpose: the main branch ruleset requires 1 approval
# with require_last_push_approval, so every extra PR is manual review work.
version: 2

updates:
  - package-ecosystem: cargo
    directory: "/"
    schedule:
      interval: weekly
      day: monday
      time: "06:00"
      timezone: America/Sao_Paulo
    open-pull-requests-limit: 5
    labels: ["dependencies", "rust"]
    commit-message:
      prefix: "chore(deps)"
      prefix-development: "chore(deps-dev)"
      include: scope
    cooldown:
      default-days: 3
      semver-major-days: 7
    groups:
      # One PR for every patch/minor bump. Majors stay individual so they get a
      # real review.
      cargo-minor-patch:
        applies-to: version-updates
        patterns: ["*"]
        update-types: ["minor", "patch"]
      # Tightly coupled stacks: bumping one without the others breaks the build,
      # so they must land together even on a major.
      tokio-stack:
        applies-to: version-updates
        patterns: ["tokio", "tokio-*", "tower", "tower-*", "hyper", "hyper-*", "axum", "axum-*"]
      rustls-stack:
        applies-to: version-updates
        patterns: ["rustls", "rustls-*", "webpki*", "ring", "reqwest"]
      crypto-stack:
        applies-to: version-updates
        patterns: ["sha2", "hmac", "digest", "argon2", "chacha20poly1305", "aead", "crypto-common", "getrandom"]
      cargo-security:
        applies-to: security-updates
        patterns: ["*"]

  - package-ecosystem: npm
    directory: "/web"
    schedule:
      interval: weekly
      day: monday
      time: "06:00"
      timezone: America/Sao_Paulo
    open-pull-requests-limit: 5
    labels: ["dependencies", "javascript"]
    commit-message:
      prefix: "chore(deps)"
      prefix-development: "chore(deps-dev)"
      include: scope
    versioning-strategy: increase
    cooldown:
      default-days: 3
      semver-major-days: 7
    groups:
      react-stack:
        applies-to: version-updates
        patterns: ["react", "react-dom", "@types/react", "@types/react-dom", "react-router*"]
      vite-toolchain:
        applies-to: version-updates
        patterns: ["vite", "@vitejs/*", "vitest", "@vitest/*", "typescript", "oxlint", "jsdom", "@testing-library/*"]
      npm-minor-patch:
        applies-to: version-updates
        patterns: ["*"]
        update-types: ["minor", "patch"]
      npm-security:
        applies-to: security-updates
        patterns: ["*"]

  # Required for SHA pinning to stay maintainable: Dependabot rewrites the SHA
  # and the trailing "# vX.Y.Z" comment together.
  - package-ecosystem: github-actions
    directory: "/"
    schedule:
      interval: weekly
      day: monday
      time: "06:00"
      timezone: America/Sao_Paulo
    open-pull-requests-limit: 5
    labels: ["dependencies", "github-actions"]
    commit-message:
      prefix: "ci(deps)"
      include: scope
    cooldown:
      default-days: 3
    groups:
      actions-all:
        applies-to: version-updates
        patterns: ["*"]
```

### 4.12 `deny.toml`

```toml
# cargo-deny 0.18+ schema. The old version / unlicensed / copyleft /
# allow-osi-fsf-free / default keys were removed upstream and now error out, so
# they are deliberately absent.

[graph]
all-features = true
targets = [
  "x86_64-unknown-linux-gnu",   # Fly / Docker runtime
  "aarch64-unknown-linux-gnu",  # multiarch GHCR image
  "x86_64-pc-windows-msvc",     # dev machines
  "aarch64-apple-darwin",
]

[output]
feature-depth = 1

[advisories]
db-urls = ["https://github.com/rustsec/advisory-db"]
# A yanked crate in the lockfile is a real signal: either the author pulled a
# broken release or a compromised one.
yanked = "deny"
unmaintained = "workspace"
ignore = [
  # { id = "RUSTSEC-0000-0000", reason = "no fixed release yet; not reachable from our code paths" },
]

[licenses]
# Exact set observed across the current dependency graph. Anything new forces a
# conscious decision instead of silently entering an AGPL-3.0-only codebase.
# Deliberately absent and therefore blocked: GPL-2.0-only (incompatible, no
# "or later"), BUSL-1.1, SSPL-1.0, Elastic-2.0, CC-BY-NC-*. MPL-2.0 and LGPL-*
# are compatible with AGPL but are left out on purpose so pulling one requires
# a reviewed PR: MPL is per-file copyleft and changes the distribution
# obligations of the Docker image.
allow = [
  "AGPL-3.0-only",             # quark itself
  "MIT",
  "Apache-2.0",
  "Apache-2.0 WITH LLVM-exception",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "ISC",
  "Unicode-3.0",
  "Unlicense",
  "Zlib",
  "BSL-1.0",
  "CDLA-Permissive-2.0",
]
confidence-threshold = 0.93
unused-allowed-license = "warn"
include-dev = false
include-build = true

[licenses.private]
ignore = false

[bans]
multiple-versions = "warn"
# A wildcard version means "whatever the registry serves today", which defeats
# the point of a reviewed lockfile.
wildcards = "deny"
allow-wildcard-paths = true
highlight = "all"
deny = [
  { crate = "openssl", reason = "quark is rustls-only (reqwest/sqlx/hyper all use rustls-tls)" },
  { crate = "openssl-sys", reason = "see openssl" },
  { crate = "native-tls", reason = "see openssl" },
  { crate = "time", version = "<0.3", reason = "RUSTSEC-2020-0071" },
]
skip = [
  # Fill from the first local `cargo deny check bans` run, not from guesses.
]

[sources]
unknown-registry = "deny"
unknown-git = "deny"
allow-registry = ["https://github.com/rust-lang/crates.io-index"]
allow-git = []
```

### 4.13 `.github/workflows/supply-chain.yml`

```yaml
name: Supply chain

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]
  schedule:
    # RustSec advisories land continuously; a nightly run surfaces them without
    # breaking unrelated pull requests.
    - cron: "17 5 * * *"
  workflow_dispatch:

permissions:
  contents: read

concurrency:
  group: supply-chain-${{ github.ref }}
  cancel-in-progress: true

jobs:
  # Deterministic checks: they only depend on the lockfile, so they gate PRs.
  cargo-deny:
    name: cargo-deny (${{ matrix.checks }})
    runs-on: ubuntu-latest
    timeout-minutes: 15
    strategy:
      fail-fast: false
      matrix:
        checks: [bans, licenses, sources]
    steps:
      - uses: actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09 # v5
        with:
          persist-credentials: false
      - uses: EmbarkStudios/cargo-deny-action@3c6349835b2b7b196a839186cb8b78e02f7b5f25 # v2
        with:
          command: check ${{ matrix.checks }}
          arguments: --all-features

  # Time-dependent check: informational on PRs, authoritative on schedule/main.
  cargo-advisories:
    name: cargo-deny advisories
    runs-on: ubuntu-latest
    timeout-minutes: 15
    steps:
      - uses: actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09 # v5
        with:
          persist-credentials: false
      - uses: EmbarkStudios/cargo-deny-action@3c6349835b2b7b196a839186cb8b78e02f7b5f25 # v2
        continue-on-error: ${{ github.event_name == 'pull_request' }}
        with:
          command: check advisories
          arguments: --all-features
```

### 4.14 `.github/workflows/dependency-review.yml`

```yaml
name: Dependency review

on:
  pull_request:
    branches: [main]

permissions:
  contents: read

jobs:
  review:
    runs-on: ubuntu-latest
    timeout-minutes: 10
    permissions:
      contents: read
      pull-requests: write   # only for comment-summary-in-pr
    steps:
      - uses: actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09 # v5
        with:
          persist-credentials: false
      - uses: actions/dependency-review-action@a1d282b36b6f3519aa1f3fc636f609c47dddb294 # v5
        with:
          fail-on-severity: high
          comment-summary-in-pr: on-failure
          # Mirrors deny.toml but also covers npm and GitHub Actions, which
          # cargo-deny cannot see. Denylist rather than allowlist: the npm graph
          # is 500+ packages wide and an allowlist would be pure noise.
          deny-licenses: >-
            GPL-2.0-only,
            GPL-2.0,
            SSPL-1.0,
            BUSL-1.1,
            Elastic-2.0,
            CC-BY-NC-4.0,
            CC-BY-NC-SA-4.0
```

### 4.15 `.github/workflows/ci.yml` (reescrito)

Confira os SHAs no dia de escrever o arquivo (`gh api repos/actions/checkout/git/ref/tags/v5 --jq .object.sha`); os abaixo foram resolvidos em 2026-07-24.

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

# Least privilege by default. Jobs that need more raise it locally.
permissions:
  contents: read

env:
  CARGO_TERM_COLOR: always
  CARGO_NET_RETRY: 3
  CARGO_INCREMENTAL: 0

concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: ${{ github.event_name == 'pull_request' }}

jobs:
  check:
    runs-on: ubuntu-latest
    timeout-minutes: 45
    permissions:
      contents: read
    services:
      valkey:
        image: valkey/valkey:8
        ports: ["6379:6379"]
        options: >-
          --health-cmd "valkey-cli ping" --health-interval 5s --health-timeout 3s --health-retries 5
      postgres:
        image: postgres:16
        env:
          POSTGRES_PASSWORD: postgres
        ports: ["5432:5432"]
        options: >-
          --health-cmd pg_isready --health-interval 5s --health-timeout 3s --health-retries 5
      clickhouse:
        image: clickhouse/clickhouse-server:24
        ports: ["8123:8123"]
        options: >-
          --health-cmd "wget -q -O - http://localhost:8123/ping || exit 1" --health-interval 5s --health-timeout 3s --health-retries 10
    steps:
      - uses: actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09 # v5
        with:
          persist-credentials: false

      - name: Install Rust (stable + rustfmt + clippy)
        run: |
          rustup toolchain install stable --profile minimal --component rustfmt --component clippy
          rustup default stable

      - uses: actions/cache@0057852bfaa89a56745cba8c7296529d2fc39830 # v4
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            target
          key: ${{ runner.os }}-cargo-${{ hashFiles('**/Cargo.lock') }}

      - run: cargo fmt --check
      - run: cargo clippy --all-targets -- -D warnings

      # Guards against a PR that edits Cargo.toml without refreshing Cargo.lock,
      # which would let cargo-deny and the dependency graph go stale.
      - name: Lockfile is up to date
        run: cargo metadata --locked --format-version 1 > /dev/null

      - run: cargo build --release
      - name: Test
        run: cargo test
        env:
          QUARK_TEST_VALKEY_URL: redis://127.0.0.1:6379
          QUARK_TEST_DATABASE_URL: postgres://postgres:postgres@127.0.0.1:5432/postgres
          QUARK_TEST_CLICKHOUSE_URL: http://127.0.0.1:8123

  web:
    runs-on: ubuntu-latest
    timeout-minutes: 20
    permissions:
      contents: read
    defaults:
      run:
        working-directory: web
    steps:
      - uses: actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09 # v5
        with:
          persist-credentials: false
      - uses: actions/setup-node@a0853c24544627f65ddf259abe73b1d18a591444 # v5
        with:
          node-version: "20"
          cache: npm
          cache-dependency-path: web/package-lock.json
      - run: npm ci
      - run: npm run lint
      - run: npm run typecheck
      - run: npm run test
      - run: npm run build

  # Deploy do backend no Fly (quark-prod) so quando um push na main passa nos
  # dois jobs acima. O front deploya sozinho pelo Cloudflare Pages.
  deploy-backend:
    needs: [check, web]
    if: github.event_name == 'push' && github.ref == 'refs/heads/main'
    runs-on: ubuntu-latest
    timeout-minutes: 30
    permissions:
      contents: read
    environment: production
    concurrency:
      group: deploy-backend
      cancel-in-progress: false
    steps:
      - uses: actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09 # v5
        with:
          persist-credentials: false
      # Pinned to a master SHA because the subpath has no usable release tag.
      # Dependabot cannot bump a branch pin, so revisit this by hand.
      - uses: superfly/flyctl-actions/setup-flyctl@ed8efb33836e8b2096c7fd3ba1c8afe303ebbff1 # master @ 2026-07
      - run: flyctl deploy --remote-only
        env:
          FLY_API_TOKEN: ${{ secrets.FLY_API_TOKEN }}
```

### 4.16 `Dockerfile` (reescrito)

```dockerfile
# syntax=docker/dockerfile:1

# ---- planner: extract the dependency graph only ----
FROM rust:1.89-bookworm AS chef
WORKDIR /app
RUN cargo install cargo-chef --locked

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ---- build ----
# The official Rust image ships gcc, which heed needs to compile LMDB (C) and
# link it statically into the binary.
FROM chef AS build
COPY --from=planner /app/recipe.json recipe.json
# This layer is only invalidated when Cargo.lock/Cargo.toml change. It is what
# turns a 12 minute release into a 3 minute one.
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --bin quark

# ---- runtime ----
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends gosu ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -r -u 10001 quark \
    && mkdir -p /data \
    && chown quark:quark /data
COPY --from=build /app/target/release/quark /usr/local/bin/quark
COPY docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh
# QUARK_KEY is deliberately NOT set here. Provide it as a secret.
ENV QUARK_ADDR=0.0.0.0:8080 \
    QUARK_DATA=/data
EXPOSE 8080
VOLUME ["/data"]
ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
CMD ["quark"]
```

Duas notas: `ca-certificates` faltava e sem ele os webhooks de saída e o discovery de OIDC falham com erro de TLS opaco (remova só se você confirmar que o `reqwest` está usando `webpki-roots` embutido). Não adicionei `HEALTHCHECK` porque exigiria `wget` na slim; o healthcheck fica no orquestrador batendo em `/healthz`.

Depois que a primeira imagem sair, pinar as bases por digest (`docker buildx imagetools inspect rust:1.89-bookworm --format '{{.Manifest.Digest}}'`) e só então habilitar o bloco `docker` no `dependabot.yml`.

### 4.17 `.dockerignore` (reescrito)

```
/target
/data
.git
.github
.superpowers
*.mdb
node_modules
web/node_modules
web/dist
docs
e2e
benches
signatures
*.png
*.log
docker-compose*.yml
fly.toml*
```

### 4.18 `CHANGELOG.md`

```markdown
**English** · [Português](CHANGELOG.PT_BR.md)

# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

Versioning is SemVer with 0.x semantics, the Cargo convention:

- `0.MINOR.0` is a breaking release. It can change the HTTP API, rename or
  remove a `QUARK_*` variable, or change the on-disk format.
- `0.MINOR.PATCH` is compatible features and bug fixes.
- `0.MINOR.0-rc.N` is a pre-release and never gets the `latest` image tag.

The public contract covered by those numbers is: the HTTP API (`/`, `/:code`,
`/:code/stats`, `/admin/*`), the `QUARK_*` variables, the LMDB on-disk format
and the Postgres migrations, and the webhook payload and signature. The Rust
library surface in `src/lib.rs`, the admin panel HTML, and the ClickHouse table
layout are not covered.

## [Unreleased]

## [0.2.0] - 2026-07-24

First tagged release and first published container image. Everything below has
been in `main` since the project started; this entry marks the point where it
became installable.

### Added
- Multi-arch container image on GHCR (`linux/amd64`, `linux/arm64`) with SLSA
  build provenance.
- `quark --version` and a `version` field in `/healthz`.
- Short codes computed from a calibrated reduced-round ARX permutation, with no
  code index on disk.
- Pluggable backends: LMDB or Postgres for storage, in-process cache with an
  optional Valkey L2 tier, embedded analytics or ClickHouse.
- Admin panel: link CRUD, tags, QR codes, UTM builder, per-link stats.
- Auth: admin token, API tokens with scopes, OIDC login, SSO domain mapping.
- Multi-tenancy: tenants, custom domains, invites.
- Signed webhooks following the Standard Webhooks spec.
- A/B testing, redirect rules, deep linking, password protected links,
  conversion forwarding, sheet and CSV import from Bitly, Kutt and YOURLS.

### Security
- Private vulnerability reporting, a written security policy, and a supply chain
  pipeline: cargo-deny, dependency review, CodeQL, and SHA-pinned workflows.

[Unreleased]: https://github.com/lucasolopes/quark/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/lucasolopes/quark/releases/tag/v0.2.0
```

### 4.19 `CHANGELOG.PT_BR.md`

```markdown
[English](CHANGELOG.md) · **Português**

# Changelog

Todas as mudanças relevantes do projeto ficam registradas aqui. O formato segue
o [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/).

O versionamento é SemVer com semântica 0.x, a convenção do Cargo:

- `0.MINOR.0` é uma release com quebra. Pode mudar a API HTTP, renomear ou
  remover uma variável `QUARK_*`, ou mudar o formato em disco.
- `0.MINOR.PATCH` é funcionalidade compatível e correção de bug.
- `0.MINOR.0-rc.N` é pré-release e nunca recebe a tag `latest` da imagem.

O contrato público coberto por esses números é: a API HTTP (`/`, `/:code`,
`/:code/stats`, `/admin/*`), as variáveis `QUARK_*`, o formato do LMDB em disco
e as migrações do Postgres, e o payload e a assinatura dos webhooks. A superfície
de biblioteca em `src/lib.rs`, o HTML do painel e o layout das tabelas do
ClickHouse não estão cobertos.

## [Não lançado]

## [0.2.0] - 2026-07-24

Primeira tag e primeira imagem publicada. Tudo abaixo já estava na `main` desde
o começo do projeto; esta entrada marca o ponto em que ele virou instalável.

### Adicionado
- Imagem multi-arquitetura no GHCR (`linux/amd64`, `linux/arm64`) com
  provenance SLSA do build.
- `quark --version` e um campo `version` no `/healthz`.
- Códigos curtos calculados por uma permutação ARX de rodadas reduzidas
  calibrada, sem índice de códigos em disco.
- Backends plugáveis: LMDB ou Postgres para armazenamento, cache em processo com
  camada L2 opcional em Valkey, analytics embutido ou ClickHouse.
- Painel admin: CRUD de links, tags, QR code, montador de UTM, estatísticas por
  link.
- Autenticação: token de admin, tokens de API com escopo, login OIDC, mapeamento
  de domínio SSO.
- Multi-tenancy: tenants, domínios próprios, convites.
- Webhooks assinados seguindo a spec Standard Webhooks.
- Teste A/B, regras de redirect, deep linking, links protegidos por senha,
  encaminhamento de conversão, importação de planilha e CSV do Bitly, Kutt e
  YOURLS.

### Segurança
- Relato privado de vulnerabilidade, política de segurança escrita, e um
  pipeline de supply chain: cargo-deny, dependency review, CodeQL e workflows
  pinados por SHA.

[Não lançado]: https://github.com/lucasolopes/quark/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/lucasolopes/quark/releases/tag/v0.2.0
```

### 4.20 `.github/workflows/release.yml`

```yaml
name: Release

on:
  push:
    tags:
      - "v*"

permissions:
  contents: read

env:
  REGISTRY: ghcr.io
  IMAGE_NAME: lucasolopes/quark

jobs:
  # Cheap gate (~15s). Fails the release BEFORE spending 15 minutes of build
  # time if the tag does not match Cargo.toml or the CHANGELOG has no section.
  guard:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    outputs:
      version: ${{ steps.v.outputs.version }}
      prerelease: ${{ steps.v.outputs.prerelease }}
    steps:
      - uses: actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09 # v5
        with:
          persist-credentials: false

      - name: Tag, Cargo.toml and CHANGELOG must agree
        id: v
        run: |
          set -euo pipefail
          tag="${GITHUB_REF_NAME}"
          version="${tag#v}"

          cargo_version="$(sed -n '/^\[package\]/,/^\[/p' Cargo.toml \
            | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)"

          if [ "$cargo_version" != "$version" ]; then
            echo "::error::tag $tag does not match Cargo.toml version $cargo_version"
            exit 1
          fi

          if ! grep -qF "## [$version]" CHANGELOG.md; then
            echo "::error::CHANGELOG.md has no '## [$version]' section"
            exit 1
          fi

          case "$version" in
            *-*) prerelease=true ;;
            *)   prerelease=false ;;
          esac

          echo "version=$version"       >> "$GITHUB_OUTPUT"
          echo "prerelease=$prerelease" >> "$GITHUB_OUTPUT"

      - name: Cargo.lock must be committed and current
        run: cargo metadata --locked --format-version 1 > /dev/null

  # One job per architecture on a NATIVE runner. No QEMU: emulating a release
  # build with lto=true over sqlx/clickhouse/reqwest/argon2/heed turns 12 minutes
  # into 90+. Do not add docker/setup-qemu-action here.
  build:
    needs: guard
    strategy:
      fail-fast: false
      matrix:
        include:
          - platform: linux/amd64
            runner: ubuntu-24.04
          - platform: linux/arm64
            runner: ubuntu-24.04-arm
    runs-on: ${{ matrix.runner }}
    timeout-minutes: 60
    permissions:
      contents: read
      packages: write
    steps:
      - name: Normalize platform name
        run: |
          platform="${{ matrix.platform }}"
          echo "PLATFORM_PAIR=${platform//\//-}" >> "$GITHUB_ENV"

      - uses: actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09 # v5
        with:
          persist-credentials: false

      # Labels and annotations only. Tags are applied in the merge job.
      - name: Docker metadata
        id: meta
        uses: docker/metadata-action@c299e40c65443455700f0fdfc63efafe5b349051 # v5
        with:
          images: ${{ env.REGISTRY }}/${{ env.IMAGE_NAME }}
          labels: |
            org.opencontainers.image.title=quark
            org.opencontainers.image.description=Self-hosted URL shortener in Rust whose short codes are computed from a keyed permutation
            org.opencontainers.image.licenses=AGPL-3.0-only
            org.opencontainers.image.source=https://github.com/lucasolopes/quark
            org.opencontainers.image.documentation=https://github.com/lucasolopes/quark#readme

      - uses: docker/setup-buildx-action@8d2750c68a42422c14e847fe6c8ac0403b4cbd6f # v3

      - uses: docker/login-action@c94ce9fb468520275223c153574b00df6fe4bcc9 # v3
        with:
          registry: ${{ env.REGISTRY }}
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}

      - name: Build and push by digest
        id: build
        uses: docker/build-push-action@10e90e3645eae34f1e60eeb005ba3a3d33f178e8 # v6
        with:
          context: .
          platforms: ${{ matrix.platform }}
          labels: ${{ steps.meta.outputs.labels }}
          annotations: ${{ steps.meta.outputs.annotations }}
          provenance: false   # attest-build-provenance does this properly
          sbom: false
          outputs: type=image,name=${{ env.REGISTRY }}/${{ env.IMAGE_NAME }},push-by-digest=true,name-canonical=true,push=true
          # Per-architecture scope. Without it the two jobs overwrite each
          # other's cache and every release starts cold.
          cache-from: type=gha,scope=release-${{ env.PLATFORM_PAIR }}
          cache-to: type=gha,mode=max,scope=release-${{ env.PLATFORM_PAIR }}

      - name: Export digest
        run: |
          mkdir -p /tmp/digests
          digest="${{ steps.build.outputs.digest }}"
          touch "/tmp/digests/${digest#sha256:}"

      # Unique name per matrix leg: upload-artifact@v4 rejects duplicate names.
      - uses: actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02 # v4
        with:
          name: digests-${{ env.PLATFORM_PAIR }}
          path: /tmp/digests/*
          if-no-files-found: error
          retention-days: 1

  merge:
    needs: [guard, build]
    runs-on: ubuntu-24.04
    timeout-minutes: 20
    permissions:
      contents: read
      packages: write
      id-token: write       # OIDC for Sigstore
      attestations: write
    outputs:
      digest: ${{ steps.index.outputs.digest }}
    steps:
      - uses: actions/download-artifact@d3f86a106a0bac45b974a628896c90dbdf5c8093 # v4
        with:
          path: /tmp/digests
          pattern: digests-*
          merge-multiple: true

      - uses: docker/setup-buildx-action@8d2750c68a42422c14e847fe6c8ac0403b4cbd6f # v3

      - name: Docker metadata (tags)
        id: meta
        uses: docker/metadata-action@c299e40c65443455700f0fdfc63efafe5b349051 # v5
        with:
          images: ${{ env.REGISTRY }}/${{ env.IMAGE_NAME }}
          # latest=false so a release candidate never grabs `latest`.
          # {{major}} is deliberately absent: in 0.x it would be `0`, which
          # would lump mutually incompatible releases under one tag.
          flavor: |
            latest=false
          tags: |
            type=semver,pattern={{version}}
            type=semver,pattern={{major}}.{{minor}}
            type=raw,value=latest,enable=${{ needs.guard.outputs.prerelease == 'false' }}
          labels: |
            org.opencontainers.image.title=quark
            org.opencontainers.image.licenses=AGPL-3.0-only
            org.opencontainers.image.source=https://github.com/lucasolopes/quark

      - uses: docker/login-action@c94ce9fb468520275223c153574b00df6fe4bcc9 # v3
        with:
          registry: ${{ env.REGISTRY }}
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}

      - name: Create and push manifest list
        working-directory: /tmp/digests
        run: |
          set -euo pipefail
          docker buildx imagetools create \
            $(jq -cr '.tags | map("-t " + .) | join(" ")' <<< "$DOCKER_METADATA_OUTPUT_JSON") \
            $(printf '${{ env.REGISTRY }}/${{ env.IMAGE_NAME }}@sha256:%s ' *)

      - name: Read index digest
        id: index
        run: |
          set -euo pipefail
          digest="$(docker buildx imagetools inspect \
            ${{ env.REGISTRY }}/${{ env.IMAGE_NAME }}:${{ steps.meta.outputs.version }} \
            --format '{{ json .Manifest }}' | jq -r .digest)"
          echo "digest=$digest" >> "$GITHUB_OUTPUT"

      - name: Verify both platforms are in the index
        run: |
          set -euo pipefail
          out="$(docker buildx imagetools inspect \
            ${{ env.REGISTRY }}/${{ env.IMAGE_NAME }}:${{ steps.meta.outputs.version }})"
          echo "$out"
          echo "$out" | grep -q 'linux/amd64'
          echo "$out" | grep -q 'linux/arm64'

      # Attest the INDEX, not each per-architecture manifest. The digest users
      # reach through the tag is the index digest; attesting the leaves makes
      # `gh attestation verify` fail for them.
      - name: Attest build provenance
        uses: actions/attest-build-provenance@ef244123eb79f2f7a7e75d99086184180e6d0018 # v2
        with:
          subject-name: ${{ env.REGISTRY }}/${{ env.IMAGE_NAME }}
          subject-digest: ${{ steps.index.outputs.digest }}
          push-to-registry: true

  release:
    needs: [guard, merge]
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    permissions:
      contents: write
    steps:
      - uses: actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09 # v5
        with:
          fetch-depth: 0
          persist-credentials: false

      - name: Extract the section for this version from the CHANGELOG
        env:
          VERSION: ${{ needs.guard.outputs.version }}
          DIGEST: ${{ needs.merge.outputs.digest }}
        run: |
          set -euo pipefail
          awk -v v="## [$VERSION]" '
            index($0, v) == 1 { grab = 1; next }
            grab && /^## \[/   { exit }
            grab               { print }
          ' CHANGELOG.md > /tmp/notes.md

          cat >> /tmp/notes.md <<EOF

          ## Container image

          \`\`\`
          docker pull ghcr.io/lucasolopes/quark:$VERSION
          \`\`\`

          Platforms: \`linux/amd64\`, \`linux/arm64\`.
          Digest: \`$DIGEST\`

          Verify the build provenance:

          \`\`\`
          gh attestation verify oci://ghcr.io/lucasolopes/quark:$VERSION --repo lucasolopes/quark
          \`\`\`

          Português: [CHANGELOG.PT_BR.md](https://github.com/lucasolopes/quark/blob/$GITHUB_REF_NAME/CHANGELOG.PT_BR.md)
          EOF

      - name: Create the GitHub Release
        uses: softprops/action-gh-release@72f2c25fcb47643c292f7107632f7a47c1df5cd8 # v2
        with:
          body_path: /tmp/notes.md
          generate_release_notes: true
          prerelease: ${{ needs.guard.outputs.prerelease }}
          make_latest: ${{ needs.guard.outputs.prerelease == 'false' }}
```

---

## 5. COMANDOS gh / API

```bash
# 1. Descricao do repo. Coloca os tokens de busca (self-hosted, URL shortener, Rust)
#    nas primeiras 5 palavras e a unica claim que nenhum concorrente pode fazer no fim.
gh repo edit lucasolopes/quark --description "Self-hosted URL shortener in Rust: the short code is computed from a keyed permutation, never stored. One ~1 MB binary, zero runtime dependencies."

# 2. Os 20 topics (limite do GitHub). Ordenados por valor de descoberta.
gh repo edit lucasolopes/quark --add-topic url-shortener,self-hosted,rust,shortener,link-shortener,urlshortener,shorten-urls,short-url,axum,tokio,clickhouse,webhooks,qr-code,ab-testing,single-binary,lmdb,deep-linking,link-management,bitly-alternative,link-analytics

# 3. Desligar a Wiki (esta vazia, nao passa por CLA nem por review, e sinaliza abandono).
gh repo edit lucasolopes/quark --enable-wiki=false

# 4. Secret scanning: falta push protection, non-provider patterns e validity checks.
gh api -X PATCH repos/lucasolopes/quark \
  -F 'security_and_analysis[secret_scanning][status]=enabled' \
  -F 'security_and_analysis[secret_scanning_push_protection][status]=enabled' \
  -F 'security_and_analysis[secret_scanning_non_provider_patterns][status]=enabled' \
  -F 'security_and_analysis[secret_scanning_validity_checks][status]=enabled'

# 5. Label que falta para o dependabot (dependencies, rust e javascript ja existem).
gh label create github-actions --color 000000 --description "GitHub Actions workflows"

# 6. Baixar o ruleset atual para editar (a API de rulesets SUBSTITUI o objeto inteiro, nao faz merge).
gh api repos/lucasolopes/quark/rulesets/19673028 > ruleset.json
#    Acrescente ao array .rules, mantendo tudo que ja esta la:
#    {"type":"required_status_checks","parameters":{"strict_required_status_checks_policy":false,
#     "required_status_checks":[{"context":"check"},{"context":"web"},
#       {"context":"cargo-deny (bans)"},{"context":"cargo-deny (licenses)"},
#       {"context":"cargo-deny (sources)"},{"context":"review"}]}}

# 7. Aplicar o ruleset editado. Rode SO depois que os workflows da fase 2 tiverem rodado
#    ao menos uma vez, senao os contexts nao existem e todo PR fica pendente para sempre.
gh api -X PUT repos/lucasolopes/quark/rulesets/19673028 --input ruleset.json

# 8. Tag ruleset para v*: ruleset de branch NAO cobre tags, e o release.yml tem packages: write.
gh api -X POST repos/lucasolopes/quark/rulesets \
  -f name='protect release tags' -f target='tag' -f enforcement='active' \
  -F 'conditions[ref_name][include][]=refs/tags/v*' \
  -F 'conditions[ref_name][exclude][]=' \
  -F 'rules[][type]=deletion' -F 'rules[][type]=non_fast_forward' \
  -F 'bypass_actors[][actor_id]=5' -F 'bypass_actors[][actor_type]=RepositoryRole' -F 'bypass_actors[][bypass_mode]=always'

# 9. Conferencia do estado depois da fase 1 e 2.
gh repo view lucasolopes/quark --json description,repositoryTopics,hasWikiEnabled,homepageUrl
gh api repos/lucasolopes/quark --jq '.security_and_analysis'
gh api repos/lucasolopes/quark/code-scanning/default-setup
gh api repos/lucasolopes/quark/private-vulnerability-reporting
gh api repos/lucasolopes/quark/secret-scanning/alerts --jq '[.[] | {number, state, secret_type, validity, html_url}]'

# 10. Resolver os SHAs das actions no dia de escrever os workflows (nao confie nos daqui).
for a in actions/checkout actions/setup-node actions/cache actions/upload-artifact actions/download-artifact actions/github-script actions/dependency-review-action actions/attest-build-provenance github/codeql-action EmbarkStudios/cargo-deny-action ossf/scorecard-action docker/setup-buildx-action docker/login-action docker/metadata-action docker/build-push-action softprops/action-gh-release contributor-assistant/github-action; do \
  echo -n "$a "; gh api "repos/$a/releases/latest" --jq '.tag_name' 2>/dev/null; done

# 11. FASE 3, primeira release. Depois que Cargo.toml estiver em 0.2.0 e o CHANGELOG tiver a secao.
git tag -a v0.2.0 -m "quark v0.2.0" && git push origin v0.2.0
gh run watch

# 12. Depois da primeira release: conferir o package e torna-lo publico.
gh api /user/packages/container/quark --jq '{name, visibility, repository: .repository.full_name}'
#     Visibilidade de package de conta pessoal NAO tem endpoint REST. E pela UI:
#     https://github.com/users/lucasolopes/packages/container/quark/settings
#     -> Danger Zone -> Change visibility -> Public
#     E na mesma tela, Manage Actions access: o repo lucasolopes/quark precisa estar com role Write.

# 13. Verificar como anonimo.
docker logout ghcr.io
docker buildx imagetools inspect ghcr.io/lucasolopes/quark:0.2.0
gh attestation verify oci://ghcr.io/lucasolopes/quark:0.2.0 --repo lucasolopes/quark

# 14. FASE 4, quando a landing/Pages existir.
gh repo edit lucasolopes/quark --homepage https://lucasolopes.github.io/quark

# 15. FASE 4, opcional, depois que a fila de alertas do CodeQL estiver limpa.
gh api -X PATCH repos/lucasolopes/quark/code-scanning/default-setup \
  -f state=configured -f query_suite=security-extended \
  -F 'languages[]=actions' -F 'languages[]=javascript-typescript' -F 'languages[]=rust'
```

---

## 6. ARMADILHAS

**A1. O `cla.yml` vai falhar em todo PR do Dependabot, e não é por causa de commit assinado.** Em eventos disparados pelo Dependabot (inclusive `pull_request_target`), o `GITHUB_TOKEN` é rebaixado para read-only e os secrets do Actions não são expostos. O `cla.yml` usa `secrets.PERSONAL_ACCESS_TOKEN` e chama `issues.createComment`: dá 403. O `allowlist: dependabot[bot]` resolve a checagem de CLA, não impede o job de rodar e quebrar. Correção obrigatória **antes** de commitar o `dependabot.yml`: `if: github.actor != 'dependabot[bot]'` no nível do job.

**A2. `require_last_push_approval` + `dismiss_stale_reviews_on_push` viram loop com automerge.** Você aprova o PR do bot, o bot faz rebase, sua aprovação some. Por isso o agrupamento agressivo no `dependabot.yml`: poucos PRs, revisão em lote uma vez por semana logo depois da segunda-feira. **Não crie bypass de ruleset para `dependabot[bot]`**: merge sem review de um bot que escreve `Cargo.lock` é exatamente o vetor que você quer fechar.

**A3. Aplicar `required_status_checks` antes dos workflows existirem trava tudo.** Os contexts (`check`, `web`, `cargo-deny (bans)`, ...) precisam ter rodado ao menos uma vez na main. Se você adicionar um context com nome errado, todo PR fica "Expected — Waiting for status" para sempre e a única saída é o bypass de admin. Copie os nomes exatos de `gh api repos/lucasolopes/quark/commits/main/check-runs --jq '.check_runs[].name'`.

**A4. `superfly/flyctl-actions/setup-flyctl@master` executa código arbitrário no job que carrega `FLY_API_TOKEN`.** Qualquer commit no master da Superfly roda no seu runner com acesso ao token de deploy de produção. No incidente do `tj-actions/changed-files`, o atacante moveu todas as tags de v1 a v45 para um commit malicioso e atingiu 23 mil repos; quem estava pinado por SHA não foi afetado. Pinar por SHA é urgente. Como o subpath não tem tag utilizável, o Dependabot não consegue bumpar esse pin: anote para revisitar à mão a cada trimestre.

**A5. Tag ruleset não existe e `release.yml` tem `packages: write`.** Rulesets de branch não cobrem tags. Hoje qualquer pessoa com write no repo empurra `v9.9.9` e publica `latest` no GHCR. Crie o tag ruleset (comando 8) **antes** de mergear o `release.yml`, não depois.

**A6. `denied: permission_denied` na primeira release.** O erro mais provável do dia da tag, e a mensagem não ajuda. Não é o `permissions:` do workflow: o package novo nasce sem o repositório listado em *Manage Actions access* com role Write. Corrige na UI de settings do package.

**A7. `latest` apontando para um release candidate.** `flavor: latest=auto` do metadata-action marca latest em qualquer tag semver, inclusive `v0.3.0-rc.1`. Por isso `latest=false` + `type=raw` condicional no `prerelease`.

**A8. Cache do buildx compartilhado entre arquiteturas.** Sem `scope=release-linux-amd64` / `scope=release-linux-arm64`, os dois jobs escrevem na mesma entrada e cada release começa fria nos dois arcos. Sintoma: o tempo nunca cai depois da primeira release.

**A9. QEMU voltando pela porta dos fundos.** Se alguém acrescentar `docker/setup-qemu-action` "por segurança", o buildx pode escolher o emulador em vez do runner arm64 nativo e o build de 4 min vira 90 min sem aviso, até estourar o timeout. O comentário no YAML existe por isso.

**A10. Attestation no manifest por arquitetura em vez do índice.** Se o `attest-build-provenance` for chamado dentro do job de matrix, o digest que o usuário alcança pela tag (o do índice) fica sem attestation e `gh attestation verify` falha. Você só descobre quando alguém reclama.

**A11. `.dockerignore` deixando `node_modules` da raiz entrar.** Faz o `COPY . .` invalidar a camada de build toda vez que você roda `npm install` local. Com cargo-chef pesa menos, mas o crate final ainda recompila.

**A12. Sem `ca-certificates` na `bookworm-slim`, webhooks de saída e discovery de OIDC falham com erro de TLS que parece bug de rede.** Confirme se o `reqwest` está usando `webpki-roots` (embutido) ou o store do sistema antes de decidir remover a linha.

**A13. A imagem do GHCR e o deploy do Fly divergem.** O `ci.yml` faz `flyctl deploy --remote-only`, que rebuilda o Dockerfile no Fly. Produção roda um binário que nunca passou pelo pipeline de release nem foi atestado. Depois da v0.2.0, trocar por `flyctl deploy --image ghcr.io/lucasolopes/quark:<tag>`.

**A14. `blank_issues_enabled: false` com contact_links apontando para categorias que não existem.** Crie Q&A e Ideas em Discussions **antes** de mergear o `config.yml`, senão o único caminho que sobra para quem tem uma pergunta é um 404.

**A15. `Cargo.toml` esquecido na versão antiga.** Falha silenciosa: a imagem sai com `--version` mentindo. O job `guard` custa 15 segundos e elimina a classe inteira, mas só funciona se a task 3.1 (`--version`) tiver sido feita.

**A16. `awesome-selfhosted` conta 4 meses a partir do primeiro *release*, não do primeiro commit.** O repo foi criado em 2026-07-12. Sem tag, o relógio nem começou. Submeter antes de ~novembro/2026 pega resposta automática de fechamento. E o aviso deles é explícito: contribuição gerada por LLM que não respeita as guidelines resulta em ban, então escreva o `software/quark.yml` à mão e use a grafia de licença deles (`AGPL-3.0`, sem o `-only`).

**A17. Não ligue "Require review from Code Owners" no ruleset.** Combinado com `require_last_push_approval` e a impossibilidade de aprovar o próprio PR, você fica dependente do bypass de admin em todo PR seu, o que anula a regra. O CODEOWNERS serve só para o auto-request.