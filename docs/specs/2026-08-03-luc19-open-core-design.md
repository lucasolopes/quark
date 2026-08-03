# LUC-19: modelo open-core do quark (licença e separação OSS/proprietário)

Documento de decisão. A LUC-19 fecha na decisão e no arcabouço mínimo, não na
migração completa do código. A execução por fases está no fim, e cada fase vira
uma issue própria.

## 1. Situação hoje

O repo é AGPL-3.0-only puro (`LICENSE`), um único crate Rust (`Cargo.toml`, sem
workspace) mais o painel em `web/`. Não existe separação de licença nem de
diretório entre o que é self-host e o que é cloud.

A linha de corte, porém, já existe no código como um flag de runtime:

- `QUARK_MULTI_TENANT` é lido uma vez no boot (`src/main.rs:143`) e desce como
  `multi_tenant: bool` para o store, o router e os workers.
- `src/tenant.rs` documenta a intenção: modo OSS tem exatamente um tenant
  (`DEFAULT_TENANT`), modo cloud tem muitos.
- 16 arquivos ramificam nesse flag. Os mais carregados são
  `src/store/postgres.rs` (21 ocorrências), `src/main.rs` (19) e
  `src/api/invites.rs` (7).

Dois fatos que mudam o custo da decisão:

1. **O CLA já autoriza relicenciamento.** `CLA.md` seção 3 concede ao mantenedor
   o direito de "licenciar e relicenciar Suas Contribuições sob quaisquer
   termos", incluindo licenças proprietárias e uma edição comercial hospedada.
   Não é preciso alterar o CLA nem coletar assinatura nova. Isso é o que torna o
   open-core juridicamente viável: só o titular do copyright pode combinar
   código proprietário com o próprio código AGPL, e o CLA coloca o Lucas nessa
   posição para as contribuições de terceiros.
2. **O README já promete algo que o repo não cumpre.** `README.md:323` afirma que
   a edição cloud multi-tenant "é uma oferta proprietária separada, fora deste
   core AGPL". Hoje ela está dentro do core AGPL. Ou o texto muda, ou o código
   muda. Esta spec escolhe mudar o código e ajustar o texto para o recorte real.

## 2. Benchmarks

| Projeto | Licença do core | Onde fica o pago | Mecanismo |
|---|---|---|---|
| **Cal.com** | AGPLv3 | `packages/features/ee/` | Licença comercial na pasta, license key para self-host |
| **Dub** (concorrente direto) | AGPLv3 | `apps/web/app/(ee)` e mais um caminho | `ee/LICENSE.md` separado |
| **PostHog** | MIT | `ee/` | `ee/LICENSE` própria, ausente no build hobby |
| **Chatwoot** | MIT | `enterprise/` | Remover a pasta devolve um build 100% MIT |
| **Plausible** | AGPLv3 (CE) | features de operar em escala (sites API, CRM, SSO) | source-available sem direito de uso |
| **GitLab** | MIT (CE) | `ee/` | `FOSS_ONLY=1` ou apagar `ee/` faz o build virar CE |
| **OpenObserve** | AGPL-3.0 | `enterprise/` | **cargo feature `enterprise`**, Rust, binário único |
| **n8n** | Sustainable Use License (fair-code, não é OSI) | arquivos `.ee.` / dirs `.ee` | licença separada `LICENSE_EE.md` |
| **Sentry** | FSL (vira Apache 2.0 em 2 anos) | n/a, o modelo é a licença | anti-free-riding por prazo |

O que esses casos têm em comum e vale copiar:

- **Um repo só.** Ninguém sério mantém dois repositórios espelhados. O GitLab
  tentou e voltou atrás na 12.3.
- **Uma pasta, uma licença.** O arquivo de licença dentro da pasta é o
  instrumento; o `LICENSE` da raiz só aponta a exceção.
- **Source-available, não escondido.** Cal.com resume o motivo: publicar o código
  proprietário dá transparência e mostra que não há backdoor. Nada aqui pede
  código fechado.
- **Apagar a pasta tem que funcionar.** Chatwoot e GitLab garantem isso
  explicitamente. É o teste que prova que o core continua utilizável sozinho.

O caso do **OpenObserve** é o mais próximo do quark: Rust, binário único, core
AGPL-3.0, pasta `enterprise/` sob licença comercial, ligada por uma cargo
feature `enterprise`. É o precedente que a decisão 3 segue.

## 3. Decisões

### D1. O core continua AGPL-3.0-only

Sem troca para fair-code (n8n), BUSL ou FSL.

Motivos: o AGPL já está lá e trocar licença de um projeto público é caro em
confiança; a cláusula 13 do AGPL já é a trava anti-free-riding que interessa a
um encurtador (quem rodar um quark modificado como serviço para terceiros tem
que publicar as modificações); o concorrente direto (Dub) usa exatamente AGPL
mais `ee/`, então essa posição é a esperada no mercado; e AGPL é licença OSI,
o que fair-code e FSL não são, o que importa para um projeto ainda em fase de
adoção.

Descartado: FSL/BUSL, que resolvem o problema de free-riding por prazo em vez de
por escopo. Fazem sentido para um produto com tração e um clone de nuvem
grande na cola, não para o estágio atual.

### D2. O corte é single-tenant contra operar-como-serviço

O `Cal.com` chama isso de singleplayer contra multiplayer, e é o mesmo eixo que
`src/tenant.rs` já descreve. Formulado como regra de decisão:

> Fica no core tudo que uma organização precisa para rodar o quark para si
> mesma. Vai para a EE tudo que só serve para operar o quark como serviço para
> terceiros: criar e cobrar de contas alheias, provisionar identidade por
> cliente e vender white-label.

Consequência importante e deliberada: **o isolamento entre tenants continua no
core**. `TenantId`, `DEFAULT_TENANT`, o predicado `WHERE tenant_id` e o
`FORCE ROW LEVEL SECURITY` do `src/store/postgres.rs` ficam AGPL. Três razões:
já estão publicados, são primitiva de segurança e não de monetização, e
extraí-los significaria fatiar o hot path do redirect, que é restrição fixa do
projeto. O que vira EE é a **administração** de múltiplos tenants, não a
capacidade de haver mais de um.

### D2.1. O login OIDC de instância única fica no core

Decidido em 2026-08-03, depois de levantar a questão. A regra do D2 aplicada a
identidade: um issuer único configurado por env é o que uma organização precisa
para si mesma, então fica AGPL. Config de IdP por tenant, discovery por domínio
de e-mail e provisionamento de realm são o que só serve para operar como
serviço, então são EE.

O argumento de "PostHog e Plausible cobram por SSO" não se transfere: aqueles
produtos têm conta local com senha de graça embaixo, e cobram pelo andar de
cima (SAML, SCIM, diretório). O quark não tem login local, as únicas rotas de
sessão são OIDC (`src/api/router.rs:91-93`), e o admin token é um segredo
compartilhado sem usuário nomeado. Fechar o OIDC não seria cobrar por SSO, seria
cobrar para ter usuários nomeados, e exigiria construir login local antes só
para não regredir.

Pesa também que o AGPL já concedido é irrevogável: a v0.4.1 com OIDC está
publicada para sempre e pode ser forkada. Fechar feature já lançada rende pouco
e custa confiança. Se identidade virar alvo de monetização além disso, o alvo
certo é feature que ainda não existe (SCIM, política de sessão, log de
auditoria), não a que já saiu.

### D3. Mecanismo: pasta `ee/` mais cargo feature, sem workspace novo

- Código de servidor proprietário em `src/ee/`, com `src/ee/LICENSE`.
- `#[cfg(feature = "ee")]` no `mod ee` do `src/lib.rs`. Feature **não** default.
- `cargo build` sozinho produz o binário Community. `cargo build --features ee`
  produz o binário do cloud. Apagar `src/ee/` mantém o build Community verde, e
  isso vira teste de CI.
- Painel: `web/src/ee/` com `web/src/ee/LICENSE`, ligado por
  `VITE_QUARK_EE=1`. As telas EE já são lazy-imports de rota, então o corte cai
  no `router.tsx`.

Descartado: crate separado num cargo workspace. Custo alto (reorganizar o crate
inteiro, `AppState` cruza a fronteira em quase todo handler) para o mesmo
resultado prático nesta fase. Pode virar refino depois se `src/ee/` crescer.

Descartado: repositório privado separado. Perde o benefício de transparência e
duplica CI, review e release para um mantenedor só.

Descartado: só flag de runtime, que é o que existe hoje. Não resolve nada de
licenciamento: o código proprietário continuaria distribuído sob AGPL.

### D4. Segunda licença: uma EE License curta, além do AGPL

Três instrumentos, com papéis distintos:

1. `LICENSE` na raiz, AGPL-3.0-only, com um parágrafo de exceção apontando
   `src/ee/` e `web/src/ee/`. É o modelo do Dub.
2. `src/ee/LICENSE` (e o espelho em `web/src/ee/LICENSE`): source-available,
   leitura e contribuição permitidas, uso em produção só com assinatura válida.
   A do Chatwoot é o modelo mais enxuto e é o que serve de base.
3. Licença comercial do core sob demanda, para quem quer o core sem o copyleft
   do AGPL. Isso o README já oferece e o CLA já sustenta. Continua sendo trato
   caso a caso, sem texto publicado nesta fase.

Nenhuma mudança no `CLA.md`. Entra só uma linha no `CONTRIBUTING.md`: PR que
toca `ee/` é aceito e entra sob a licença da pasta, não sob o AGPL.

### D5. Gating de feature paga: build primeiro, license key depois

Duas camadas, com prazos diferentes.

**Camada 1, build-time (agora).** O binário Community não contém o código EE.
É a garantia forte, não depende de honestidade nem de checagem em runtime.

**Camada 2, runtime (esboçada agora, implementada quando existir o primeiro
cliente self-host pagante).** Um `enum LicenseStatus { Community, Enterprise {
expires_at, seats, features } }` resolvido uma vez no boot e guardado no
`AppState`. Validação offline de um token assinado com Ed25519, chave pública
embutida no binário. O repo já tem `jsonwebtoken` e material de cripto
(`src/secretbox.rs`), então o custo é baixo quando chegar a hora.

Nesta fase entra só o seam: o enum, o campo no `AppState` sempre em `Community`
no build OSS, e um ponto único de checagem. Sem servidor de licenças, sem
telemetria, sem chamada de rede. Construir a máquina de licenças antes do
primeiro pagante é trabalho jogado fora.

Regra de degradação, decidida agora para não virar improviso depois: licença
expirada ou ausente **não derruba o serviço e não apaga dado**. Bloqueia
operação de criação nas features EE e deixa o resto em leitura. Um encurtador
que para de redirecionar porque a licença venceu é um incidente, não um modelo
de negócio.

### D6. Empacotamento e release

- A imagem publicada em GHCR continua sendo a Community, build padrão.
- O deploy do cloud passa a construir com `--features ee`. É uma linha no
  `Dockerfile` via `ARG QUARK_FEATURES` e uma no workflow de deploy.
- O CI ganha um job que compila e roda a suíte **sem** a feature (o que já é o
  default de hoje) e outro **com** a feature, para que o código EE não quebre
  sem ninguém ver.

## 4. A linha de corte, arquivo a arquivo

Esta seção foi reescrita a partir do inventário em
`docs/research/2026-08-03-luc19-inventario-oss-ee.md`, que varreu 49 rotas, 28
módulos, 115 métodos do trait `Store` e 59 variáveis de ambiente. A primeira
versão classificava por nome de arquivo e errou em três pontos, todos
corrigidos aqui.

**O código já decidiu a maior parte.** 15 das 49 rotas retornam 404 quando
`!st.multi_tenant`. Já são cloud-only em comportamento; o que falta é a licença
acompanhar o que a execução faz.

**Fica no core AGPL.** Redirect e hot path (`codec.rs`, `permute.rs`,
`domain_router.rs`), stores (LMDB e Postgres, incluindo RLS), cache, analytics,
webhooks, pixels, Sheets, Slack, tokens de API, CRUD de links, A/B, health,
senha de link, import, QR, deep links, o painel de um workspace, e o login OIDC
com um IdP único (D2.1).

**Vai para `src/ee/`: 2665 linhas de servidor, 15 rotas.**

| Hoje | Linhas | Por que é EE |
|---|---|---|
| `src/api/tenants.rs` | 505 | criar, excluir e trocar de workspace |
| `src/api/invites.rs` | 474 | convites mais os três handlers de `/admin/oidc-config`, que moram aqui e não em `tenants.rs` |
| `src/api/sso_domains.rs` | 255 | discovery por domínio de e-mail |
| `src/api/domains.rs` | 282 | múltiplos domínios com verificação DNS, decidido em 2026-08-03 |
| `src/keycloak/` | 1149 | provisionar um realm de identidade por cliente |
| futuro LUC-41 / LUC-64 / LUC-58 | - | billing, planos, white-label |

**Ficam no core, apesar de parecerem cloud.** `src/invite.rs` (22) e
`src/sso.rs` (84) são nomeados pelo trait `Store` nas assinaturas
(`src/store/mod.rs:752-798` e `:724-735`), então sair dali quebra o core.
Mesma coisa para `src/domain.rs` e `src/tenant.rs`. Movem os handlers, ficam os
tipos persistidos.

**O trait `Store` fica inteiro no core.** 46 dos 115 métodos servem entidades
que só existem de fato em cloud (tenants, memberships, invites, oidc_configs,
sso_email_domains, domains), mas é um trait só, implementado por `LmdbStore` e
`PostgresStore`, ambos core. Dividir exigiria um supertrait opcional e
genéricos por toda a superfície de handler para ganhar zero: dado persistido não
é o que se vende, e o schema é público de qualquer jeito. A edição Community
carrega o schema multi-tenant completo e não sabe administrá-lo.

**Vai para `web/src/ee/`.** `routes/Members.tsx`, `routes/SsoDomains.tsx`,
`routes/OidcProvider.tsx`, `routes/AcceptInvite.tsx`, `routes/Domains.tsx`,
`components/WorkspaceSwitcher.tsx`, `components/DeleteWorkspaceDialog.tsx`,
`app/WorkspaceGate.tsx`, com os testes junto.

**Variáveis de ambiente Enterprise: 11 de 59.** `QUARK_MULTI_TENANT`,
`QUARK_TENANT_DOMAIN_SUFFIX` e os nove `QUARK_KEYCLOAK_*`. No build Community
passam a ser ignoradas, então a `docs/CONFIGURATION.md` precisa marcá-las,
senão o operador configura e fica esperando efeito. `QUARK_ADMIN_HOST` fica no
core: restringir `/admin/*` a um host serve qualquer deploy.

**Fica pendente de recorte fino: `src/api/oidc_login.rs` (600 linhas).** O
arquivo mistura o login OIDC de instância única (core) com a resolução de
config por tenant e a rota `?org=` (EE). Não dá para mover o arquivo inteiro
para nenhum dos lados. A proposta é deixá-lo no core nesta fase e extrair a
resolução por tenant atrás de um trait (`TenantIdpResolver`), cuja
implementação real mora em `src/ee/` e cujo fallback no core devolve a config
única. Isso é execução, e vira issue própria.

## 5. Como o corte é feito no código

Decidir a pasta é a parte fácil. O que faz a separação ser real é o conjunto de
soldas que permite o core compilar com `src/ee/` ausente. Sem elas, "apagar a
pasta funciona" é promessa vazia. São cinco pontos, todos no core.

**5.1. `AppState` não pode nomear tipos EE.** Hoje `src/api/mod.rs:126-131`
carrega `keycloak: Option<Arc<dyn crate::keycloak::KeycloakAdmin>>`,
`keycloak_base_url`, `oidc_tenants` e `tenant_domain_suffix`. Com o módulo fora,
o struct deixa de compilar. Esses campos viram um agregado só:

```rust
pub struct AppState {
    // ... campos do core
    #[cfg(feature = "ee")]
    pub ee: crate::ee::EeState,
}
```

O inventário reduziu esse agregado a dois campos: `EeState` carrega apenas
`keycloak` e `keycloak_base_url`, que são os únicos que nomeiam um tipo que sai
do core. `tenant_domain_suffix` fica, porque `src/api/links.rs:502,532` o lê no
caminho de criação; `oidc_tenants` fica, porque é cache sobre o store e mantê-lo
evita `cfg` dentro do `oidc_login.rs`; `dns` e `host_router` ficam, o segundo
por estar no hot path do redirect.

**5.2. O `router()` ganha um ponto de injeção único.** Hoje é um builder plano
com mais de 60 `.route(...)`, e as rotas cloud estão intercaladas
(`src/api/router.rs:95-141`). As rotas EE saem do core e passam a ser montadas
por uma chamada só:

```rust
let r = /* rotas do core */;
#[cfg(feature = "ee")]
let r = crate::ee::api::mount(r);
```

Um ponto de injeção, não um `cfg` por rota. É o equivalente em Rust do que o
GitLab faz com os módulos que ele prepende no `ee/`.

**5.3. O prelúdio interno precisa virar público-no-crate.** O `CLAUDE.md`
documenta a convenção: os submódulos de `src/api/` usam `use super::*` sobre o
namespace plano de `mod.rs`. Um módulo em `src/ee/api/` não tem esse `super`.
`mod.rs` passa a expor `pub(crate) mod prelude` com os mesmos reexports, os
módulos do core seguem com `use super::*` e os módulos EE usam
`use crate::api::prelude::*`. O `CLAUDE.md` é atualizado junto, senão a
convenção documentada e a real divergem.

**5.4. O boot cloud do `main.rs` vira uma chamada.** Backfill de Keycloak,
provisionamento de realm e seed de subdomínio automático estão espalhados em
`src/main.rs`. Viram `#[cfg(feature = "ee")] crate::ee::boot(&state).await?`.

**5.5. Os testes seguem o código.** `tests/workspace_it.rs`,
`tests/invites_it.rs`, `tests/sso_domains_it.rs` e a parte por-tenant do
`tests/oidc_config_it.rs` exercitam código EE, e `tests/common/mod.rs` monta o
`AppState`, que passa a ter dois formatos. Os arquivos EE ganham
`#![cfg(feature = "ee")]` no topo, e o builder do `TestState` monta o campo `ee`
sob o mesmo `cfg`.

**A prova.** Um job de CI que executa, nessa ordem:

```bash
rm -rf src/ee web/src/ee
cargo build && cargo test
cd web && npm run build
```

Se passa, a separação existe. Se não passa, o `ee/` é decoração e o core não é
utilizável sozinho. Chatwoot e GitLab garantem exatamente isso, e é o teste que
impede a fronteira de apodrecer com o tempo.

## 6. O que muda no repo

Novos: `src/ee/LICENSE`, `src/ee/mod.rs`, `web/src/ee/LICENSE`,
`docs/LICENSING.md` e o twin `docs/LICENSING.PT_BR.md`.

Alterados: `LICENSE` (parágrafo de exceção), `README.md` e `README.PT_BR.md` (a
seção de licença passa a descrever o recorte real, corrigindo a promessa atual),
`CONTRIBUTING.md` e twin (uma linha sobre PRs em `ee/`), `Cargo.toml` (feature
`ee`), `src/lib.rs` (`#[cfg(feature = "ee")] pub mod ee;`), `Dockerfile` (ARG de
features), `.github/workflows/ci.yml` (job com a feature ligada e o teste de
"apagar `src/ee/` e continuar compilando").

## 7. Fases de execução

- **Fase 0, esta spec.** Decisão aprovada e commitada. Fecha a LUC-19.
- **Fase 1, arcabouço.** Textos de licença, `src/ee/` vazio com a feature, jobs
  de CI, docs e README corrigidos. Nenhum código de produto se move. É a fase
  que torna a promessa do README verdadeira.
- **Fase 2, mudança dos módulos separáveis.** `tenants`, `invites`,
  `sso_domains`, `keycloak`. Movimento mecânico, sem mudança de comportamento.
- **Fase 3, o recorte do `oidc_login`.** O trait descrito acima.
- **Fase 4, seam de licença.** `LicenseStatus` no `AppState` e o ponto único de
  checagem, ainda sem validação real.

Fase 1 é pré-requisito da LUC-41 (billing), que já nasce EE.

## 8. Riscos e questões em aberto

1. **Texto da EE License: aprovado pelo Lucas em 2026-08-03.** O risco foi
   levantado e a decisão foi seguir com o texto como está. Ele parte da licença
   do Chatwoot, que é curta e já roda em produção em outro projeto. Nada aqui é
   aconselhamento jurídico, e revisar com advogado continua sendo possível a
   qualquer momento sem mexer no código, já que o arquivo é isolado
   (`src/ee/LICENSE` e o espelho no painel).
2. **AGPL sobre o próprio cloud.** O quark cloud roda código AGPL modificado
   servindo terceiros, então a cláusula 13 se aplica ao core. Na prática o
   core do cloud é o mesmo do repo público, o que já satisfaz a obrigação, mas
   isso precisa continuar verdadeiro: uma correção feita só no deploy do cloud
   e não publicada é violação da própria licença.
3. **Marca.** Fora do escopo desta issue, mas vizinho o bastante para registrar:
   "Quarkus" é marca da Red Hat para um framework Java conhecido, e o projeto
   usa `quarkus.com.br`. Rastreado na LUC-144, a resolver antes da landing page
   (LUC-40) e de qualquer registro de marca.
4. **Contribuição externa em `ee/`.** Aceitar PR numa pasta proprietária afasta
   parte dos contribuidores. É o custo conhecido do modelo, e todos os
   benchmarks pagam ele.

## Fontes

- [n8n, Sustainable Use License](https://github.com/n8n-io/n8n/blob/master/LICENSE.md) e [docs](https://docs.n8n.io/sustainable-use-license/)
- [Chatwoot, enterprise/LICENSE](https://github.com/chatwoot/chatwoot/blob/develop/enterprise/LICENSE) e [Managing Enterprise Edition Features](https://developers.chatwoot.com/self-hosted/enterprise-edition)
- [PostHog, ee/LICENSE](https://github.com/PostHog/posthog/blob/master/ee/LICENSE)
- [Cal.com, Changing to AGPLv3 and introducing the Enterprise Edition](https://cal.com/blog/changing-to-agplv3-and-introducing-enterprise-edition) e [packages/features/ee/LICENSE](https://github.com/calcom/cal.com/blob/main/packages/features/ee/LICENSE)
- [Dub, LICENSE.md](https://github.com/dubinc/dub/blob/main/LICENSE.md)
- [Plausible, Introducing Plausible Community Edition](https://plausible.io/blog/community-edition)
- [GitLab, A single codebase for Community and Enterprise Edition](https://about.gitlab.com/blog/a-single-codebase-for-gitlab-community-and-enterprise-edition/)
- [OpenObserve, Cargo.toml com a feature enterprise](https://github.com/openobserve/openobserve/blob/main/Cargo.toml) e [por que trocaram Apache por AGPL](https://openobserve.ai/blog/what-are-apache-gpl-and-agpl-licenses-and-why-openobserve-moved-from-apache-to-agpl/)
- [Sentry, Introducing the Functional Source License](https://blog.sentry.io/introducing-the-functional-source-license-freedom-without-free-riding/) e [fsl.software](https://fsl.software/)
- [Open-core model, Wikipedia](https://en.wikipedia.org/wiki/Open-core_model)
