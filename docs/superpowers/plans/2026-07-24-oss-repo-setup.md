# Setup open source do repositório: plano de implementação

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Levar o `lucasolopes/quark` ao padrão de repositório open source profissional: metadados, health files bilíngues, automação de dependências, verificação de supply chain, CI que de fato bloqueia merge, e a primeira release publicada como imagem multiarch no GHCR.

**Architecture:** Quatro fases entregues num PR único a partir do worktree `../quark-oss-setup` (branch `chore/oss-repo-setup`, criado de `origin/main`). Os commits são separados por fase para que reverter uma fase isolada continue possível. Configuração de repositório que não vive no PR (topics, toggles, rulesets) roda via `gh` em momentos específicos em relação ao merge, marcados nas tarefas.

**Tech Stack:** Rust 2021 (axum, tokio), React + TypeScript + Vite em `web/`, GitHub Actions, Docker buildx, cargo-deny, Dependabot.

## Global Constraints

- **Worktree:** todo trabalho de arquivo acontece em `C:/Users/L-SALDANHA/pessoal/quark-oss-setup`. Nunca tocar em `C:/Users/L-SALDANHA/pessoal/quark`, que é a árvore de trabalho do dono.
- **Cargo:** o `cargo` não está no PATH. Usar `~/.cargo/bin/cargo.exe`.
- **Bilinguismo:** todo arquivo novo voltado ao usuário ou contribuidor tem gêmeo `X.PT_BR.md`. O arquivo em inglês começa com `**English** · [Português](X.PT_BR.md)` e o gêmeo com `[English](X.md) · **Português**`.
- **Prosa:** sem travessão em (—), inglês técnico direto, pt-BR natural. Regra `avoid-ai-writing` do `CLAUDE.md`.
- **Conteúdo verbatim:** o conteúdo literal de cada arquivo está em `docs/research/2026-07-24-oss-readiness.md`, referenciado por número de seção em cada tarefa. Ler a seção antes de escrever o arquivo. Onde este plano diverge da pesquisa, **este plano vence** (as divergências estão marcadas).
- **Pinagem:** todo `uses:` em workflow fica pinado por SHA de 40 caracteres, com comentário `# vX.Y.Z` na mesma linha. Resolver os SHAs no dia com o laço do comando 10 da seção 5 da pesquisa. Nunca copiar SHA do documento de pesquisa sem resolver.
- **Repo:** `lucasolopes/quark`. Ruleset da `main`: id `19673028`.
- **Commits:** mensagem em pt-BR, no imperativo, prefixo convencional (`docs:`, `ci:`, `chore:`, `feat:`, `fix:`). Terminar com a linha `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`.

## Divergências em relação à spec

Encontradas ao ler o código, depois que a spec foi escrita. A spec assumiu o que a pesquisa afirmou.

1. **Não existe `/healthz`.** A rota é `GET /health` e o handler `health()` em `src/api/router.rs:14` devolve a string estática `"ok"`. Existe teste em `tests/api_it.rs:2167` e o `fly.toml:37` usa esse path no healthcheck. Trocar a resposta por JSON seria breaking change num endpoint público. **Decisão:** manter `/health` respondendo `"ok"` e expor a versão num header `X-Quark-Version`. Não quebra teste, não quebra healthcheck, não gasta um code do namespace com uma rota `/version` nova.

---

## Estrutura de arquivos

**Criar:**

| Arquivo | Responsabilidade |
| --- | --- |
| `SECURITY.md` + `SECURITY.PT_BR.md` | Política de disclosure, apontando ao PVR |
| `CODE_OF_CONDUCT.md` + `CODE_OF_CONDUCT.PT_BR.md` | Contributor Covenant 2.1 |
| `.github/ISSUE_TEMPLATE/bug.yml` | Form de bug com campos de versão e de backend |
| `.github/ISSUE_TEMPLATE/feature.yml` | Form de proposta |
| `.github/ISSUE_TEMPLATE/config.yml` | Desliga issue em branco, aponta contact links |
| `.github/PULL_REQUEST_TEMPLATE.md` | Só o que CI e bot de CLA não checam |
| `.github/CODEOWNERS` | Auto-request de review |
| `.github/dependabot.yml` | Version updates de cargo, npm e actions |
| `deny.toml` | Política de licença, advisory e bans |
| `.github/workflows/supply-chain.yml` | Roda cargo-deny |
| `.github/workflows/dependency-review.yml` | Diff de dependência em PR |
| `.github/workflows/release.yml` | Build multiarch e publicação no GHCR |
| `CHANGELOG.md` + `CHANGELOG.PT_BR.md` | Keep a Changelog, com semântica 0.x no topo |
| `docs/assets/panel.png` | Screenshot do painel para o README |

**Modificar:**

| Arquivo | Mudança |
| --- | --- |
| `.github/workflows/cla.yml` | `if:` contra o Dependabot, actions pinadas |
| `.github/workflows/ci.yml` | `permissions:`, pins por SHA, `needs: [check, web]` |
| `CONTRIBUTING.md` + gêmeo | Reescrita completa |
| `README.md` + gêmeo | Heading órfão, quick start, tabela comparativa, badges, benchmarks |
| `src/main.rs` | Flag `--version` |
| `src/api/router.rs` | Header `X-Quark-Version` no `/health` |
| `Cargo.toml` | Versão `0.2.0` e metadados de crate |
| `Dockerfile` | cargo-chef, pin por digest, `ca-certificates` |
| `.dockerignore` | Excluir `node_modules` da raiz e artefatos |

---

## FASE 1: metadados e health files

### Task 1: Configuração do repositório

Não produz arquivo. É configuração via API, e vem primeiro porque as categorias de Discussions precisam existir antes de a Task 3 mergear.

**Files:** nenhum.

**Interfaces:**
- Produces: categorias `q-a` e `ideas` em Discussions, que a Task 3 referencia em `config.yml`.

- [ ] **Step 1: Aplicar descrição, topics e wiki**

Conteúdo exato da descrição e dos 20 topics: seção 5, comandos 1 a 3 da pesquisa.

```bash
gh repo edit lucasolopes/quark --description "Self-hosted URL shortener in Rust: the short code is computed from a keyed permutation, never stored. One ~1 MB binary, zero runtime dependencies."
gh repo edit lucasolopes/quark --add-topic url-shortener,self-hosted,rust,shortener,link-shortener,urlshortener,shorten-urls,short-url,axum,tokio,clickhouse,webhooks,qr-code,ab-testing,single-binary,lmdb,deep-linking,link-management,bitly-alternative,link-analytics
gh repo edit lucasolopes/quark --enable-wiki=false --delete-branch-on-merge
```

- [ ] **Step 2: Verificar**

```bash
gh repo view lucasolopes/quark --json description,repositoryTopics,hasWikiEnabled,deleteBranchOnMerge
```

Esperado: descrição preenchida, 20 topics, `hasWikiEnabled: false`, `deleteBranchOnMerge: true`.

- [ ] **Step 3: Ligar o que falta no secret scanning**

```bash
gh api -X PATCH repos/lucasolopes/quark \
  -F 'security_and_analysis[secret_scanning_push_protection][status]=enabled' \
  -F 'security_and_analysis[secret_scanning_non_provider_patterns][status]=enabled' \
  -F 'security_and_analysis[secret_scanning_validity_checks][status]=enabled'
```

- [ ] **Step 4: Verificar**

```bash
gh api repos/lucasolopes/quark --jq '.security_and_analysis'
```

Esperado: `secret_scanning`, `secret_scanning_push_protection`, `secret_scanning_non_provider_patterns` e `secret_scanning_validity_checks` todos `enabled`.

- [ ] **Step 5: Criar as categorias de Discussions**

Não há endpoint REST para criar categoria. Pela UI, em `github.com/lucasolopes/quark/discussions/categories`, garantir que existam uma categoria de formato Q&A com slug `q-a` e uma de formato Open-ended com slug `ideas`.

- [ ] **Step 6: Verificar que os slugs respondem**

```bash
gh api graphql -f query='{repository(owner:"lucasolopes",name:"quark"){discussionCategories(first:20){nodes{slug name}}}}' --jq '.data.repository.discussionCategories.nodes[].slug'
```

Esperado: a lista inclui `q-a` e `ideas`. Se não incluir, a Task 3 vai produzir contact links que dão 404.

- [ ] **Step 7: Criar a label que falta**

```bash
gh label create github-actions --color 000000 --description "GitHub Actions workflows"
```

Esperado: sucesso, ou "already exists", que também serve.

---

### Task 2: Política de segurança e código de conduta

**Files:**
- Create: `SECURITY.md`, `SECURITY.PT_BR.md`, `CODE_OF_CONDUCT.md`, `CODE_OF_CONDUCT.PT_BR.md`

**Interfaces:**
- Produces: `SECURITY.md` e `CODE_OF_CONDUCT.md`, referenciados pelo `config.yml` da Task 3 e pelo `CONTRIBUTING.md` da Task 4.

- [ ] **Step 1: Escrever os quatro arquivos**

Conteúdo verbatim: seções 4.1, 4.2 e 4.3 da pesquisa. Pontos que não podem ser alterados:

- Canal único de reporte é o formulário de advisory do PVR, em
  `https://github.com/lucasolopes/quark/security/advisories/new`. Nenhum e-mail.
- Versões suportadas declaram apenas a última minor, que é o honesto num pré-1.0.
- O Code of Conduct é o texto do Contributor Covenant 2.1 verbatim, com o campo
  de contato preenchido com `https://github.com/contact/report-abuse`.
- Header bilíngue conforme a constraint global.

- [ ] **Step 2: Verificar os links internos**

```bash
cd C:/Users/L-SALDANHA/pessoal/quark-oss-setup
grep -o 'https://[^)]*' SECURITY.md CODE_OF_CONDUCT.md | sort -u
```

Esperado: nenhuma URL de placeholder, nenhum `example.com`, nenhum endereço de e-mail.

- [ ] **Step 3: Verificar a troca de idioma**

```bash
head -1 SECURITY.md SECURITY.PT_BR.md CODE_OF_CONDUCT.md CODE_OF_CONDUCT.PT_BR.md
```

Esperado: cada arquivo em inglês aponta para o gêmeo PT_BR e vice-versa, sem link quebrado.

- [ ] **Step 4: Commit**

```bash
git add SECURITY.md SECURITY.PT_BR.md CODE_OF_CONDUCT.md CODE_OF_CONDUCT.PT_BR.md
git commit -m "docs: politica de seguranca e codigo de conduta"
```

---

### Task 3: Templates de issue e PR, CODEOWNERS

Depende da Task 1 step 5: as categorias de Discussions precisam existir.

**Files:**
- Create: `.github/ISSUE_TEMPLATE/bug.yml`, `.github/ISSUE_TEMPLATE/feature.yml`, `.github/ISSUE_TEMPLATE/config.yml`, `.github/PULL_REQUEST_TEMPLATE.md`, `.github/CODEOWNERS`

**Interfaces:**
- Consumes: slugs `q-a` e `ideas` das Discussions (Task 1), `SECURITY.md` (Task 2).

- [ ] **Step 1: Escrever os cinco arquivos**

Conteúdo verbatim: seções 4.4 a 4.8 da pesquisa. Pontos obrigatórios:

- `bug.yml` pergunta versão do quark, e tem dropdowns separados para backend de
  store (LMDB ou Postgres), de cache (in-memory ou Valkey) e de analytics
  (embutido ou ClickHouse). É isso que torna a triagem possível com três eixos
  plugáveis.
- O campo de versão instrui a obter com `quark --version`, que a Task 9 cria.
- `config.yml` tem `blank_issues_enabled: false` e contact links para
  Discussions Q&A, Discussions Ideas, o advisory de segurança e a pasta `docs`.
- `CODEOWNERS` tem duas linhas e atribui tudo a `@lucasolopes`.
- O `PULL_REQUEST_TEMPLATE.md` não repete o que CI e bot de CLA já checam.

- [ ] **Step 2: Validar o YAML dos forms**

```bash
cd C:/Users/L-SALDANHA/pessoal/quark-oss-setup
python -c "import yaml,sys; [yaml.safe_load(open(f,encoding='utf-8')) for f in ['.github/ISSUE_TEMPLATE/bug.yml','.github/ISSUE_TEMPLATE/feature.yml','.github/ISSUE_TEMPLATE/config.yml']]; print('yaml ok')"
```

Esperado: `yaml ok`. Um form com YAML inválido não aparece na UI e o GitHub não avisa.

- [ ] **Step 3: Conferir que os contact links batem com as categorias reais**

```bash
grep -o 'discussions/categories/[a-z-]*' .github/ISSUE_TEMPLATE/config.yml
```

Esperado: só `discussions/categories/q-a` e `discussions/categories/ideas`, exatamente os slugs verificados na Task 1 step 6.

- [ ] **Step 4: Commit**

```bash
git add .github/ISSUE_TEMPLATE .github/PULL_REQUEST_TEMPLATE.md .github/CODEOWNERS
git commit -m "docs: templates de issue e PR, CODEOWNERS"
```

---

### Task 4: CONTRIBUTING reescrito e correção do heading órfão

**Files:**
- Modify: `CONTRIBUTING.md`, `CONTRIBUTING.PT_BR.md`, `README.md`, `README.PT_BR.md`

**Interfaces:**
- Consumes: `CODE_OF_CONDUCT.md` (Task 2).

- [ ] **Step 1: Reescrever o CONTRIBUTING e o gêmeo**

Conteúdo verbatim: seções 4.9 e 4.10 da pesquisa. O arquivo atual tem 51 linhas e
precisa passar a cobrir, no mínimo: o frontend em `web/` e seus comandos, as
regras de i18n, a regra de doc bilíngue, convenção de commit, o efeito de
`require_last_push_approval` no fluxo de review, o que não será mergeado, e quem
decide. **Não** incluir seção sobre assinatura de commit: a regra foi removida do
ruleset em 2026-07-24 e a pesquisa da frente de health está desatualizada nesse ponto.

- [ ] **Step 2: Corrigir o bloco de código órfão do README**

Localizar o bloco ```bash que hoje aparece sem heading acima (por volta da linha
179 de `README.md`) e inserir um heading `## Quick start` antes dele. Espelhar em
`README.PT_BR.md`.

- [ ] **Step 3: Adicionar a linha de Code of Conduct nos dois READMEs**

Uma linha na seção de contribuição apontando para `CODE_OF_CONDUCT.md`, e o
equivalente no gêmeo apontando para `CODE_OF_CONDUCT.PT_BR.md`.

- [ ] **Step 4: Verificar que não sobrou bloco órfão**

```bash
cd C:/Users/L-SALDANHA/pessoal/quark-oss-setup
python - <<'EOF'
import re
for f in ("README.md","README.PT_BR.md"):
    lines=open(f,encoding="utf-8").read().split("\n")
    for i,l in enumerate(lines):
        if l.startswith("```") and l.strip()!="```":
            prev=[x for x in lines[max(0,i-6):i] if x.strip()]
            if not any(x.startswith("#") or x.endswith(":") or x.endswith(".") for x in prev[-2:] or [""]):
                print(f"{f}:{i+1} bloco possivelmente orfao")
print("scan ok")
EOF
```

Esperado: `scan ok` sem linha de aviso para o bloco corrigido.

- [ ] **Step 5: Commit**

```bash
git add CONTRIBUTING.md CONTRIBUTING.PT_BR.md README.md README.PT_BR.md
git commit -m "docs: reescreve CONTRIBUTING e corrige heading orfao do README"
```

---

## FASE 2: automação e supply chain

### Task 5: Blindar o cla.yml contra o Dependabot

Vem obrigatoriamente antes da Task 7. Se o `dependabot.yml` entrar primeiro, todo
PR do bot nasce com check vermelho de CLA.

**Files:**
- Modify: `.github/workflows/cla.yml`

**Interfaces:**
- Produces: `cla.yml` que pula em ator bot, condição de que a Task 7 depende.

- [ ] **Step 1: Entender a falha antes de corrigir**

Em evento disparado pelo Dependabot, inclusive `pull_request_target`, o
`GITHUB_TOKEN` é rebaixado para read-only e os secrets do Actions não são
expostos ao job. O `cla.yml` usa `secrets.PERSONAL_ACCESS_TOKEN` e chama
`issues.createComment`, o que resulta em 403. Usar `allowlist: dependabot[bot]`
resolveria a checagem de CLA mas não impede o job de rodar e falhar.

- [ ] **Step 2: Adicionar a condição no nível do job**

Em cada job do `cla.yml`, acrescentar:

```yaml
    if: github.actor != 'dependabot[bot]'
```

Se o job já tiver um `if:`, compor com `&&`, preservando a condição existente.

- [ ] **Step 3: Verificar a sintaxe**

```bash
cd C:/Users/L-SALDANHA/pessoal/quark-oss-setup
python -c "import yaml; d=yaml.safe_load(open('.github/workflows/cla.yml',encoding='utf-8')); print([ (k, v.get('if')) for k,v in d['jobs'].items() ])"
```

Esperado: todo job listado tem um `if` contendo `dependabot[bot]`.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/cla.yml
git commit -m "ci: pula o job de CLA em PR do dependabot"
```

---

### Task 6: Hardening do ci.yml

**Files:**
- Modify: `.github/workflows/ci.yml`, `.github/workflows/cla.yml`

**Interfaces:**
- Produces: os contexts `check` e `web`, cujos nomes a Task 16 usa em `required_status_checks`. Os nomes dos jobs **não podem mudar**, senão os contexts referenciados depois não existem.

- [ ] **Step 1: Resolver os SHAs atuais das actions**

```bash
for a in actions/checkout actions/setup-node actions/cache actions/dependency-review-action actions/attest-build-provenance EmbarkStudios/cargo-deny-action docker/setup-buildx-action docker/login-action docker/metadata-action docker/build-push-action softprops/action-gh-release contributor-assistant/github-action; do \
  tag=$(gh api "repos/$a/releases/latest" --jq '.tag_name' 2>/dev/null); \
  sha=$(gh api "repos/$a/commits/$tag" --jq '.sha' 2>/dev/null); \
  echo "$a@$sha # $tag"; done
```

Anotar a saída. É a fonte dos pins deste plano inteiro. Não usar SHA do documento de pesquisa.

- [ ] **Step 2: Reescrever o ci.yml**

Base: seção 4.15 da pesquisa. Mudanças obrigatórias em relação ao arquivo atual:

- `permissions: contents: read` no topo do workflow, e permissão elevada só no
  job que precisa. Hoje o arquivo não declara nada, o que significa token de
  escrita no job `web`, que roda `npm ci` com 564 pacotes e seus install scripts.
- Todo `uses:` pinado pelo SHA resolvido no step 1.
- `deploy-backend` passa a ter `needs: [check, web]`. Hoje tem `needs: check`, e
  por isso deploya com o front quebrado.
- Os nomes dos jobs `check`, `web` e `deploy-backend` ficam inalterados.

- [ ] **Step 3: Pinar as actions do cla.yml**

Trocar as referências por tag pelos SHAs resolvidos no step 1.

- [ ] **Step 4: Verificar a pinagem e as permissions**

```bash
cd C:/Users/L-SALDANHA/pessoal/quark-oss-setup
echo "--- uses sem SHA de 40 chars (esperado: vazio) ---"
grep -rhn 'uses:' .github/workflows/ | grep -v '@[0-9a-f]\{40\}'
echo "--- workflows sem permissions no topo (esperado: vazio) ---"
for f in .github/workflows/*.yml; do python -c "
import yaml,sys
d=yaml.safe_load(open('$f',encoding='utf-8'))
print('$f') if 'permissions' not in d else None"; done
echo "--- needs do deploy ---"
python -c "import yaml; print(yaml.safe_load(open('.github/workflows/ci.yml',encoding='utf-8'))['jobs']['deploy-backend']['needs'])"
```

Esperado: as duas primeiras seções vazias, e a terceira imprimindo `['check', 'web']`.

- [ ] **Step 5: Tratar o flyctl**

O `superfly/flyctl-actions/setup-flyctl@master` executa código de terceiro no job
que carrega o `FLY_API_TOKEN` de produção. É o vetor do incidente
`tj-actions/changed-files`. Resolver o SHA do master atual e pinar:

```bash
gh api repos/superfly/flyctl-actions/commits/master --jq '.sha'
```

Como o subpath não tem tag utilizável, o Dependabot não consegue bumpar esse pin.
Acrescentar comentário no YAML registrando que a revisão é manual e trimestral.

- [ ] **Step 6: Commit**

```bash
git add .github/workflows/ci.yml .github/workflows/cla.yml
git commit -m "ci: declara permissions, pina actions por SHA e faz o deploy depender do job web"
```

---

### Task 7: Dependabot

Depende da Task 5 (cla blindado) e da Task 1 step 7 (label `github-actions`).

**Files:**
- Create: `.github/dependabot.yml`

- [ ] **Step 1: Escrever o arquivo**

Conteúdo verbatim: seção 4.11 da pesquisa. Três ecossistemas: `cargo` em `/`,
`npm` em `/web`, `github-actions` em `/`. Os blocos de docker ficam de fora até o
Dockerfile estar pinado por digest, o que acontece na Task 11.

O agrupamento é agressivo de propósito: `require_last_push_approval` combinado
com `dismiss_stale_reviews_on_push` faz sua aprovação sumir a cada rebase do bot,
então o objetivo é poucos PRs revisados em lote.

- [ ] **Step 2: Validar o schema**

```bash
cd C:/Users/L-SALDANHA/pessoal/quark-oss-setup
python -c "
import yaml
d=yaml.safe_load(open('.github/dependabot.yml',encoding='utf-8'))
assert d['version']==2
print([(u['package-ecosystem'],u['directory']) for u in d['updates']])
"
```

Esperado: exatamente `[('cargo','/'), ('npm','/web'), ('github-actions','/')]`.

- [ ] **Step 3: Conferir que as labels referenciadas existem**

```bash
gh label list --limit 100 --json name --jq '.[].name' | sort > /tmp/have.txt 2>/dev/null || gh label list --limit 100 --json name --jq '.[].name' | sort
```

Esperado: `dependencies`, `rust`, `javascript` e `github-actions` presentes. Label
inexistente faz o Dependabot falhar silenciosamente ao abrir o PR.

- [ ] **Step 4: Commit**

```bash
git add .github/dependabot.yml
git commit -m "ci: ativa version updates do dependabot para cargo, npm e actions"
```

---

### Task 8: cargo-deny e dependency review

**Files:**
- Create: `deny.toml`, `.github/workflows/supply-chain.yml`, `.github/workflows/dependency-review.yml`

**Interfaces:**
- Produces: os contexts dos jobs de cargo-deny e de dependency review, cujos nomes exatos a Task 16 usa. Anotar os nomes ao final desta task.

- [ ] **Step 1: Instalar o cargo-deny**

```bash
~/.cargo/bin/cargo.exe install cargo-deny --locked
```

- [ ] **Step 2: Escrever o deny.toml**

Base: seção 4.12 da pesquisa. O ponto que importa num projeto AGPL é a seção
`[licenses]`: é o único mecanismo que avisa se entrar dependência com licença
incompatível.

- [ ] **Step 3: Rodar local e ver falhar**

```bash
cd C:/Users/L-SALDANHA/pessoal/quark-oss-setup
~/.cargo/bin/cargo.exe deny check
```

Esperado: falha, listando duplicatas reais em `bans` e possivelmente licenças não
declaradas. Isso é o esperado na primeira execução, não um erro do plano.

- [ ] **Step 4: Preencher o deny.toml com a realidade**

Acrescentar em `[bans].skip` as duplicatas que a saída do step 3 listou, e em
`[licenses].allow` as licenças legítimas que apareceram. Não usar `--allow` na
linha de comando: a política tem que estar no arquivo, versionada.

- [ ] **Step 5: Rodar de novo e ver passar**

```bash
~/.cargo/bin/cargo.exe deny check
```

Esperado: `advisories ok`, `bans ok`, `licenses ok`, `sources ok`, sem nenhuma flag extra.

- [ ] **Step 6: Escrever os dois workflows**

Conteúdo verbatim: seções 4.13 e 4.14 da pesquisa. Pinar por SHA conforme a
constraint global. Declarar `permissions:` no topo dos dois.

- [ ] **Step 7: Anotar os nomes exatos dos jobs**

```bash
cd C:/Users/L-SALDANHA/pessoal/quark-oss-setup
python -c "
import yaml
for f in ('.github/workflows/supply-chain.yml','.github/workflows/dependency-review.yml'):
    d=yaml.safe_load(open(f,encoding='utf-8'))
    for k,v in d['jobs'].items():
        print(f, k, '| name:', v.get('name'), '| matrix:', v.get('strategy',{}).get('matrix'))
"
```

Registrar a saída no corpo do PR. A Task 16 precisa dos nomes de context exatos,
e um job com matrix gera um context por combinação, com o formato `job (valor)`.

- [ ] **Step 8: Commit**

```bash
git add deny.toml .github/workflows/supply-chain.yml .github/workflows/dependency-review.yml
git commit -m "ci: adiciona cargo-deny e dependency review"
```

---

## FASE 3: release Docker no GHCR

### Task 9: Flag --version e header X-Quark-Version

Esta task usa TDD. Ela substitui o item da spec sobre `/healthz`, que não existe.

**Files:**
- Modify: `src/main.rs`, `src/api/router.rs`
- Test: `tests/api_it.rs`

**Interfaces:**
- Produces: o binário aceita `--version` e imprime `quark <versão do Cargo.toml>`. O `GET /health` passa a responder com o header `X-Quark-Version`. A Task 13 depende do `--version` para o job guard, e a Task 3 referencia o comando no bug form.

- [ ] **Step 1: Escrever o teste que falha, do header**

Em `tests/api_it.rs`, junto ao teste existente de `/health` (por volta da linha 2167):

```rust
#[tokio::test]
async fn health_exposes_version_header() {
    let app = test_app().await;
    let res = app
        .oneshot(Request::get("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let version = res
        .headers()
        .get("x-quark-version")
        .expect("header x-quark-version ausente")
        .to_str()
        .unwrap();
    assert_eq!(version, env!("CARGO_PKG_VERSION"));
}
```

Ajustar a construção do app ao helper que o teste vizinho de `/health` já usa em
`tests/api_it.rs:2167`. Ler aquele teste e copiar o padrão dele, não inventar um novo.

- [ ] **Step 2: Rodar e ver falhar**

```bash
cd C:/Users/L-SALDANHA/pessoal/quark-oss-setup
~/.cargo/bin/cargo.exe test --test api_it health_exposes_version_header
```

Esperado: FAIL com `header x-quark-version ausente`.

- [ ] **Step 3: Implementar o header**

Em `src/api/router.rs`, trocar o handler da linha 14:

```rust
pub(crate) async fn health() -> impl axum::response::IntoResponse {
    ([("x-quark-version", env!("CARGO_PKG_VERSION"))], "ok")
}
```

O corpo continua sendo exatamente `"ok"`, então o teste existente de `/health` e o
healthcheck do `fly.toml:37` seguem passando.

- [ ] **Step 4: Rodar e ver passar**

```bash
~/.cargo/bin/cargo.exe test --test api_it health
```

Esperado: PASS nos dois testes de health, o antigo e o novo.

- [ ] **Step 5: Implementar o --version**

Em `src/main.rs`, antes de qualquer inicialização de runtime ou de I/O:

```rust
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("quark {}", env!("CARGO_PKG_VERSION"));
        return;
    }
```

Posicionar dentro de `main`, na primeira linha do corpo. Se `main` for
`#[tokio::main] async fn main()`, o `return` continua válido. Se `main` devolver
`Result`, usar `return Ok(())`.

- [ ] **Step 6: Verificar o --version**

```bash
~/.cargo/bin/cargo.exe run -- --version
```

Esperado: imprime `quark 0.1.0` (ainda 0.1.0, a Task 10 sobe para 0.2.0) e sai sem
abrir socket, sem tocar no banco e sem exigir variável de ambiente nenhuma.

- [ ] **Step 7: Rodar a suíte inteira**

```bash
~/.cargo/bin/cargo.exe fmt --all
~/.cargo/bin/cargo.exe clippy --all-targets -- -D warnings
~/.cargo/bin/cargo.exe test
```

Esperado: tudo verde. Testes de Postgres, Valkey e ClickHouse são pulados sem as
variáveis de ambiente, o que é o comportamento normal.

- [ ] **Step 8: Commit**

```bash
git add src/main.rs src/api/router.rs tests/api_it.rs
git commit -m "feat: expoe a versao via flag --version e header no /health"
```

---

### Task 10: Cargo.toml para 0.2.0

**Files:**
- Modify: `Cargo.toml`, `Cargo.lock`

**Interfaces:**
- Produces: `version = "0.2.0"`, que o job guard da Task 13 compara com a tag, e que o `env!("CARGO_PKG_VERSION")` da Task 9 passa a refletir.

- [ ] **Step 1: Subir a versão e acrescentar metadados**

Em `Cargo.toml`, no bloco `[package]`, trocar `version = "0.1.0"` por
`version = "0.2.0"` e acrescentar:

```toml
repository = "https://github.com/lucasolopes/quark"
homepage = "https://github.com/lucasolopes/quark"
readme = "README.md"
keywords = ["url-shortener", "shortener", "self-hosted", "axum", "feistel"]
categories = ["web-programming", "command-line-utilities"]
```

O `keywords` do crates.io aceita no máximo 5 itens, cada um com no máximo 20
caracteres. O `categories` precisa usar slugs válidos do crates.io.

- [ ] **Step 2: Atualizar o lock e verificar**

```bash
cd C:/Users/L-SALDANHA/pessoal/quark-oss-setup
~/.cargo/bin/cargo.exe metadata --locked --format-version 1 > /dev/null && echo "metadata ok"
~/.cargo/bin/cargo.exe run -- --version
```

Esperado: `metadata ok` e a saída `quark 0.2.0`.

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: sobe a versao para 0.2.0 e adiciona metadados de crate"
```

---

### Task 11: Dockerfile e .dockerignore

**Files:**
- Modify: `Dockerfile`, `.dockerignore`

**Interfaces:**
- Produces: imagem cujo entrypoint aceita `quark --version`, que a Task 13 usa como smoke test.

- [ ] **Step 1: Verificar de onde vem a raiz de TLS**

```bash
cd C:/Users/L-SALDANHA/pessoal/quark-oss-setup
grep -n 'reqwest' Cargo.toml
~/.cargo/bin/cargo.exe tree -i webpki-roots 2>/dev/null | head -5
~/.cargo/bin/cargo.exe tree -i openssl 2>/dev/null | head -5
```

Se o `webpki-roots` aparecer, as raízes estão embutidas no binário. Se não
aparecer, a imagem final **precisa** de `ca-certificates`, senão webhook de saída
e discovery de OIDC falham com erro de TLS que parece problema de rede. Anotar a
conclusão e refletir no Dockerfile.

- [ ] **Step 2: Reescrever o .dockerignore**

Base: seção 4.17 da pesquisa. O problema concreto do arquivo atual é que o
`node_modules/` da raiz entra no contexto e invalida a camada de build toda vez
que você roda `npm install` local.

- [ ] **Step 3: Reescrever o Dockerfile**

Base: seção 4.16 da pesquisa. Requisitos:

- cargo-chef, para que a camada de dependências seja cacheada de verdade. Sem
  isso o build de release leva 10 a 15 minutos e o cache nunca esquenta.
- Imagens base pinadas por digest, não por tag. É o que torna os blocos docker do
  Dependabot úteis depois.
- `ca-certificates` conforme a conclusão do step 1.
- O Dockerfile **não** compila o front. Isso é verdade hoje e continua sendo:
  embutir o painel na imagem é mudança de rota em axum, está fora deste escopo.

- [ ] **Step 4: Build local e smoke test**

```bash
cd C:/Users/L-SALDANHA/pessoal/quark-oss-setup
docker build -t quark:local .
docker run --rm quark:local quark --version
```

Esperado: `quark 0.2.0`.

- [ ] **Step 5: Confirmar que o cache funciona**

```bash
touch src/main.rs && docker build -t quark:local .
```

Esperado: a camada de dependências vem do cache e o rebuild fica bem abaixo do
build frio. Se recompilar tudo, o cargo-chef está mal configurado.

- [ ] **Step 6: Commit**

```bash
git add Dockerfile .dockerignore
git commit -m "chore: reescreve o Dockerfile com cargo-chef e corrige o .dockerignore"
```

---

### Task 12: CHANGELOG

**Files:**
- Create: `CHANGELOG.md`, `CHANGELOG.PT_BR.md`

**Interfaces:**
- Produces: seção `## [0.2.0] - AAAA-MM-DD`, da qual a Task 13 extrai as notas da Release. O formato do heading é contrato, não estilo.

- [ ] **Step 1: Escrever os dois arquivos**

Conteúdo verbatim: seções 4.18 e 4.19 da pesquisa. Formato Keep a Changelog, com
o parágrafo de semântica 0.x no cabeçalho (substitui o `docs/VERSIONING.md`, que
foi cortado). Manter a seção `## [Unreleased]` no topo.

O heading da release precisa ser exatamente `## [0.2.0] - 2026-07-24`, com a data
real do dia da tag. O workflow da Task 13 extrai por esse padrão.

- [ ] **Step 2: Conferir o formato do heading**

```bash
cd C:/Users/L-SALDANHA/pessoal/quark-oss-setup
grep -n '^## \[0\.2\.0\] - [0-9]\{4\}-[0-9]\{2\}-[0-9]\{2\}$' CHANGELOG.md
```

Esperado: exatamente uma linha casando. Zero linhas significa que a extração das
notas da release vai sair vazia.

- [ ] **Step 3: Commit**

```bash
git add CHANGELOG.md CHANGELOG.PT_BR.md
git commit -m "docs: adiciona CHANGELOG bilingue no formato Keep a Changelog"
```

---

### Task 13: Ruleset de tag e workflow de release

O ruleset de tag é criado **antes** de o `release.yml` ser mergeado. Rulesets de
branch não cobrem tags, e o `release.yml` tem `packages: write`: existe uma
janela em que qualquer write publicaria `latest` no GHCR.

**Files:**
- Create: `.github/workflows/release.yml`

**Interfaces:**
- Consumes: `--version` (Task 9), `version = "0.2.0"` (Task 10), imagem que responde a `quark --version` (Task 11), heading `## [0.2.0] - ...` (Task 12).

- [ ] **Step 1: Criar o ruleset de tag**

```bash
gh api -X POST repos/lucasolopes/quark/rulesets \
  -f name='protect release tags' -f target='tag' -f enforcement='active' \
  -F 'conditions[ref_name][include][]=refs/tags/v*' \
  -F 'rules[][type]=deletion' -F 'rules[][type]=non_fast_forward' \
  -F 'bypass_actors[][actor_id]=5' -F 'bypass_actors[][actor_type]=RepositoryRole' -F 'bypass_actors[][bypass_mode]=always'
```

- [ ] **Step 2: Verificar**

```bash
gh api repos/lucasolopes/quark/rulesets --jq '.[] | {name, target, enforcement}'
```

Esperado: duas entradas, a `main` com target `branch` e a nova com target `tag`.

- [ ] **Step 3: Escrever o release.yml**

Conteúdo verbatim: seção 4.20 da pesquisa. Os pontos que **não** podem ser
simplificados, cada um correspondendo a uma armadilha real:

- Job `guard` comparando a tag com a versão do `Cargo.toml`. Sem ele a imagem sai
  com o `--version` mentindo, e é falha silenciosa.
- Matrix com `ubuntu-latest` e `ubuntu-24.04-arm`, ambos grátis em repo público.
  **Nunca** acrescentar `docker/setup-qemu-action`: com QEMU o buildx pode
  escolher o emulador em vez do runner arm64 nativo, e o build de 4 minutos vira
  90 sem aviso até estourar o timeout. O comentário no YAML precisa dizer isso.
- Cache do buildx com scope por arquitetura (`release-linux-amd64` e
  `release-linux-arm64`). Sem isso os dois jobs escrevem na mesma entrada e toda
  release começa fria.
- `flavor: latest=false` mais um `type=raw` condicional ao prerelease. O
  `latest=auto` marcaria latest até em `v0.3.0-rc.1`.
- `attest-build-provenance` chamado **fora** do job de matrix, sobre o digest do
  índice. Chamado dentro da matrix, o digest que o usuário alcança pela tag fica
  sem attestation e a verificação falha.
- `permissions:` mínimo no topo, com `packages: write`, `id-token: write`,
  `attestations: write` e `contents: write` só onde necessário.
- Dispara só em `on: push: tags: ['v*']`. Merge na main não pode disparar nada.

- [ ] **Step 4: Validar o YAML e a ausência de QEMU**

```bash
cd C:/Users/L-SALDANHA/pessoal/quark-oss-setup
python -c "import yaml; d=yaml.safe_load(open('.github/workflows/release.yml',encoding='utf-8')); print('trigger:', d['on' if 'on' in d else True]); print('jobs:', list(d['jobs']))"
echo "--- qemu (esperado: vazio) ---"; grep -n 'setup-qemu' .github/workflows/release.yml
echo "--- uses sem SHA (esperado: vazio) ---"; grep -n 'uses:' .github/workflows/release.yml | grep -v '@[0-9a-f]\{40\}'
```

Esperado: trigger só em tag, nenhum `setup-qemu`, nenhum `uses:` sem SHA.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci: adiciona workflow de release com imagem multiarch no GHCR"
```

---

## FASE 4: descoberta

### Task 14: README

**Files:**
- Modify: `README.md`, `README.PT_BR.md`
- Create: `docs/assets/panel.png`

- [ ] **Step 1: Capturar o screenshot do painel**

PNG com 1200px de largura, da lista de links do painel. Salvar em
`docs/assets/panel.png`. Conferir que não há dado real, e-mail, domínio de
cliente nem token visível na captura.

- [ ] **Step 2: Aplicar as mudanças estruturais**

Base: seção 4 da pesquisa, itens F1 a F7, e a fase 4 da seção 3. Em ordem:

1. Referenciar o screenshot logo abaixo dos quick links.
2. Mover `## Quick start` para logo depois dos quick links, agora com o
   `docker run ghcr.io/lucasolopes/quark:0.2.0` real.
3. Nova tabela comparativa depois do quick start, com quark, Shlink, YOURLS,
   Kutt e Dub nas linhas, e linguagem, serviços exigidos, tamanho, painel e
   licença nas colunas. A célula "serviços exigidos: nenhum" é o ponto da tabela.
4. Anglicizar a tabela de avalanche no `README.md` (hoje vaza pt-BR nas linhas
   82 a 91): `avg_avalanche`, `coverage(/40)`, `← ROUNDS chosen (diffusion closes)`.
5. Uma linha acima das tabelas de benchmark com CPU, número de cores, RAM, SO e
   versão do rustc. Nenhuma tabela deve dizer "measured on this machine" sem
   dizer qual máquina.
6. Badges: no máximo cinco, nenhum estático escrito à mão. License vira
   `img.shields.io/github/license/lucasolopes/quark`, entram release
   (`sort=semver`) e GHCR, saem "Rust 2021" e um dos dois redundantes.
7. Tabela de configuração inline reduzida de 17 para 6 variáveis, com link para
   `docs/CONFIGURATION.md`.

Espelhar tudo em `README.PT_BR.md`, mantendo o pt-BR natural lá.

- [ ] **Step 3: Verificar**

```bash
cd C:/Users/L-SALDANHA/pessoal/quark-oss-setup
echo "--- linhas do README (alvo: < 280) ---"; wc -l < README.md
echo "--- badges (alvo: <= 5) ---"; grep -c '^\[!\[\|^!\[' README.md
echo "--- pt-BR vazando no README em ingles (esperado: vazio) ---"; grep -n 'rodadas\|escolhid\|difus\|cobertura\|média' README.md
echo "--- screenshot referenciado ---"; grep -n 'docs/assets/panel.png' README.md README.PT_BR.md
```

Esperado: README abaixo de 280 linhas, no máximo 5 badges, nenhum português no
arquivo em inglês, screenshot referenciado nos dois.

- [ ] **Step 4: Commit**

```bash
git add README.md README.PT_BR.md docs/assets/panel.png
git commit -m "docs: reordena o README, adiciona quick start, screenshot e tabela comparativa"
```

---

### Task 15: Abrir o PR

**Files:** nenhum.

- [ ] **Step 1: Rodar a verificação completa antes de abrir**

```bash
cd C:/Users/L-SALDANHA/pessoal/quark-oss-setup
~/.cargo/bin/cargo.exe fmt --all --check
~/.cargo/bin/cargo.exe clippy --all-targets -- -D warnings
~/.cargo/bin/cargo.exe test
~/.cargo/bin/cargo.exe deny check
cd web && npm ci && npm run lint && npm run typecheck && npm run test && npm run build
```

Esperado: tudo verde. Se qualquer um falhar, corrigir antes de abrir o PR.

- [ ] **Step 2: Push e abrir o PR**

```bash
cd C:/Users/L-SALDANHA/pessoal/quark-oss-setup
git push -u origin chore/oss-repo-setup
gh pr create --title "chore: setup do repositorio para padrao open source profissional" --body-file -
```

No corpo do PR, incluir: o resumo por fase, a lista de nomes de context anotada
na Task 8 step 7, e a lista de passos pós-merge da Task 16, para que não se percam.

- [ ] **Step 3: Conferir o CI do próprio PR**

```bash
gh pr checks --watch
```

Esperado: todos verdes. O job de CLA precisa aparecer normalmente, já que o autor
não é o Dependabot.

---

### Task 16: Pós-merge

Só depois que o PR estiver mergeado na `main`. Nenhum destes passos pode ser feito antes.

- [ ] **Step 1: Coletar os nomes reais dos contexts**

```bash
gh api repos/lucasolopes/quark/commits/main/check-runs --jq '.check_runs[].name'
```

Copiar exatamente. Um nome errado deixa todo PR preso em "Expected, waiting for
status" para sempre, com saída só por bypass de admin.

- [ ] **Step 2: Adicionar required_status_checks ao ruleset**

A API de rulesets substitui o objeto inteiro, não faz merge. Baixar, editar,
subir:

```bash
gh api repos/lucasolopes/quark/rulesets/19673028 > ruleset.json
```

Acrescentar ao array `.rules`, preservando `deletion`, `non_fast_forward` e
`pull_request`, um objeto do tipo `required_status_checks` com
`strict_required_status_checks_policy: false` e a lista de contexts do step 1.
Não incluir `deploy-backend` nem `Cloudflare Pages`: o primeiro só roda em push
na main e o segundo é externo, então ambos deixariam PRs pendurados.

```bash
gh api -X PUT repos/lucasolopes/quark/rulesets/19673028 --input ruleset.json
gh api repos/lucasolopes/quark/rulesets/19673028 --jq '.rules[].type'
```

Esperado: a lista inclui `required_status_checks`.

- [ ] **Step 3: Validar com um PR descartável**

Abrir um PR que quebra o build de propósito e confirmar que o merge fica
bloqueado. Fechar sem mergear. É a única forma de saber que os nomes de context
estão certos antes de descobrir do jeito ruim.

- [ ] **Step 4: Publicar a v0.2.0**

```bash
cd C:/Users/L-SALDANHA/pessoal/quark
git checkout main && git pull
git tag -a v0.2.0 -m "quark v0.2.0"
git push origin v0.2.0
gh run watch
```

- [ ] **Step 5: Tornar o package público**

Não existe endpoint REST para visibilidade de package de conta pessoal. Pela UI,
em `github.com/users/lucasolopes/packages/container/quark/settings`: Danger Zone,
Change visibility, Public. Na mesma tela, em Manage Actions access, confirmar que
`lucasolopes/quark` está com role Write. É a causa do erro
`denied: permission_denied`, que é o erro mais provável do dia da tag e cuja
mensagem não ajuda em nada.

- [ ] **Step 6: Verificar como anônimo**

```bash
docker logout ghcr.io
docker buildx imagetools inspect ghcr.io/lucasolopes/quark:0.2.0
gh attestation verify oci://ghcr.io/lucasolopes/quark:0.2.0 --repo lucasolopes/quark
```

Esperado: o inspect lista `linux/amd64` e `linux/arm64`, e a verificação de
attestation responde sucesso.

- [ ] **Step 7: Conferir o estado final**

```bash
gh repo view lucasolopes/quark --json description,repositoryTopics,hasWikiEnabled
gh api repos/lucasolopes/quark --jq '.security_and_analysis'
gh api repos/lucasolopes/quark/community/profile --jq '.health_percentage, .files | keys'
```

Esperado: `health_percentage` em 100 e os arquivos de community health todos presentes.

- [ ] **Step 8: Limpar o worktree**

```bash
cd C:/Users/L-SALDANHA/pessoal/quark
git worktree remove ../quark-oss-setup
git branch -d chore/oss-repo-setup
```

---

## Fora deste plano

Registrado para não se perder, mas deliberadamente fora do escopo.

- **Fly deployando a imagem publicada.** Hoje o `ci.yml` faz
  `flyctl deploy --remote-only`, que rebuilda o Dockerfile no Fly. Produção roda
  um binário que nunca passou pelo pipeline de release nem foi atestado. Trocar
  por `flyctl deploy --image ghcr.io/lucasolopes/quark:<tag>` depois da v0.2.0.
- **Painel embutido na imagem.** É mudança de rota em axum, com conflito a
  resolver entre um `ServeDir` sob prefixo e o `/:code`. Gate de 1.0.
- **Blocos docker no Dependabot.** Só fazem sentido agora que o Dockerfile está
  pinado por digest. Acrescentar num PR de seguimento.
- **OpenSSF Scorecard.** Rodar depois que as fases 2 e 3 estiverem na main,
  senão o primeiro resultado público registra nota baixa. Badge só acima de 7.5.
- **CodeQL em `security-extended`.** Depois que a fila de alertas do suite
  default estiver limpa.
- **Divulgação.** awesome-selfhosted conta quatro meses a partir do primeiro
  release, e o repositório nasceu em 12/07/2026, então a janela abre por volta de
  novembro. O `software/quark.yml` tem que ser escrito à mão: contribuição
  gerada por LLM que ignore as guidelines deles resulta em ban. Depois disso,
  Show HN, r/selfhosted, r/rust, selfh.st e AlternativeTo.
- **Social preview e homepage.** O social preview 1280x640 é upload manual em
  Settings, e a homepage só quando existir Pages ou landing.
