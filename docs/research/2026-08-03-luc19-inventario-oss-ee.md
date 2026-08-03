# LUC-19: inventário core contra EE, item a item

Levantamento de apoio à spec `docs/specs/2026-08-03-luc19-open-core-design.md`.
Feito depois que a primeira tentativa de executar a mudança revelou que a
tabela da seção 4 da spec estava classificando por nome de arquivo, não por
dependência real, e errou três vezes.

Base do levantamento: 49 rotas HTTP, 28 módulos em `src/`, 115 métodos do trait
`Store`, 59 variáveis `QUARK_*` e as telas do painel.

## 1. Método

Classificar por intuição não escala. As regras abaixo são aplicadas nessa
ordem, e cada veredicto na tabela cita a que forçou.

- **R1, hot path.** O que o redirect toca fica no core. Restrição fixa do
  projeto, não negociável por licença.
- **R2, o trait manda.** Se o trait `Store` nomeia o tipo numa assinatura, o
  tipo fica no core. O trait é único e implementado por LMDB e por Postgres;
  tirar um tipo dali quebra o core.
- **R3, o próprio código já decidiu.** Handler que retorna 404 quando
  `!st.multi_tenant` já é cloud-only em comportamento. Isso é evidência
  objetiva, não opinião.
- **R4, uma organização para si mesma.** O que um self-hoster de um workspace
  usa fica no core.
- **R5, não se tira o que já saiu.** Feature publicada, documentada e anunciada
  fica no core, porque o AGPL já concedido é irrevogável e fechar depois rende
  pouco e custa confiança.

Quando R3 e R4 conflitam, o conflito é registrado como decisão em aberto na
seção 7, não resolvido no silêncio.

## 2. Três correções à spec

**C1. `src/invite.rs` e `src/sso.rs` não podem ir para a EE.** A spec mandava os
dois. O trait `Store` nomeia `Invite` e `SsoEmailDomain` nas assinaturas
(`src/store/mod.rs:752-798` e `:724-735`), então sair dali quebra o core (R2).
Vão para a EE apenas os handlers, `src/api/invites.rs` e
`src/api/sso_domains.rs`. Os tipos persistidos ficam.

**C2. `/admin/oidc-config` não está em `tenants.rs`.** Os três handlers
(`admin_oidc_config_get/put/delete`) estão em `src/api/invites.rs:321-...`. A
spec assumia o arquivo errado. O veredicto (EE) não muda, o arquivo sim.

**C3. Domínios próprios já são cloud-only no código, ao contrário do que a spec
afirmou.** A spec dizia que `domains` fica no core porque "serve o self-hoster
de um workspace só". Os cinco handlers de `src/api/domains.rs` retornam 404
quando `!st.multi_tenant`. Por R3 são EE hoje. Isso é uma decisão de produto e
está na seção 7.

## 3. Rotas HTTP (49)

**Core (34).** Todo o caminho público e todo o admin de um workspace só.

| Grupo | Rotas | Regra |
|---|---|---|
| Público e hot path | `/`, `/health`, `/{code}`, `/{code}/stats` | R1 |
| Deep links | `/.well-known/apple-app-site-association`, `/apple-app-site-association`, `/.well-known/assetlinks.json`, `/admin/wellknown/{name}` | R4 |
| Links | `/admin/stats`, `/admin/links`, `/admin/links/bulk`, `/admin/links/{code}`, `/admin/links/{code}/alert`, `/admin/links/{code}/analytics`, `/admin/import`, `/admin/tags`, `/admin/folders` | R4 |
| Webhooks | `/admin/webhooks`, `/admin/webhooks/{id}`, `/admin/webhooks/{id}/test` | R4 |
| Tokens e pixels | `/admin/tokens`, `/admin/tokens/{id}`, `/admin/pixels`, `/admin/pixels/{id}` | R4 |
| Integrações | `/admin/integrations/sheets/*` (5), `/admin/integrations/slack/*` (2) | R4 |
| Sessão | `/admin/login`, `/admin/callback`, `/admin/logout`, `/admin/me` | R5, com ramo EE |

**EE (15).** Todas com guarda `!st.multi_tenant` explícita no handler, ou seja,
já respondem 404 numa instalação OSS de hoje.

| Rota | Handler em | Guarda |
|---|---|---|
| `/admin/tenants` | `api/tenants.rs` | `admin_tenants_create` |
| `/admin/tenants/{id}` | `api/tenants.rs` | `admin_tenants_delete` |
| `/admin/workspace/switch` | `api/tenants.rs` | `admin_workspace_switch` |
| `/admin/oidc-config` (GET/PUT/DELETE) | `api/invites.rs` | os três handlers |
| `/admin/invites` | `api/invites.rs` | `admin_invites_create`, `_list` |
| `/admin/invites/{id}` | `api/invites.rs` | `admin_invites_delete` |
| `/admin/invites/{token}/accept` | `api/invites.rs` | `admin_invites_accept` |
| `/admin/sso-domains` | `api/sso_domains.rs` | `_list`, `_create` |
| `/admin/sso-domains/{id}` | `api/sso_domains.rs` | `_delete` |
| `/admin/sso-domains/{id}/verify` | `api/sso_domains.rs` | `_verify` |
| `/admin/sso/discover` | `api/sso_domains.rs` | `sso_discover` |
| `/admin/domains` | `api/domains.rs` | `_list`, `_create` |
| `/admin/domains/{id}` | `api/domains.rs` | `_delete` |
| `/admin/domains/{id}/verify` | `api/domains.rs` | `_verify` |
| `/admin/domains/{id}/primary` | `api/domains.rs` | `_set_primary` |

Nenhuma rota EE precisa de julgamento. O código já as separou; o que falta é a
licença acompanhar o que a execução já faz.

## 4. Módulos em `src/`

**Movem inteiros para `src/ee/` (2665 linhas).**

| Módulo | Linhas | Por quê |
|---|---|---|
| `api/tenants.rs` | 505 | administrar workspaces alheios (R3) |
| `api/invites.rs` | 474 | convidar terceiros e config de IdP por tenant (R3) |
| `api/sso_domains.rs` | 255 | discovery por domínio de e-mail (R3) |
| `api/domains.rs` | 282 | ver decisão em aberto 7.1 (R3) |
| `keycloak/client.rs` mais `keycloak/mod.rs` | 1149 | provisionar realm por cliente |

**Ficam no core com um ramo EE.** São os pontos onde a fronteira passa dentro do
arquivo, e por isso a Fase 3 da spec existe.

| Módulo | O que é EE dentro dele |
|---|---|
| `api/oidc_login.rs` (600) | o braço `?org=` (já guardado por `multi_tenant`) e o booleano `sso_provisioning`, que só lê `keycloak.is_some()` |
| `api/guard.rs` (184) | `admin_guard` deriva escopos por papel só em cloud |
| `api/links.rs` (1447) | `default_domain_id`, `primary_link_host` e `resolve_host_route` ramificam em cloud, e estão no hot path (R1) |
| `store/postgres.rs` (3700+) | RLS e transação por tenant, R1 e R2 |
| `main.rs` (895) | boot de Keycloak e seed de subdomínio, vira uma chamada só |

**Ficam no core inteiros, apesar de "cheirarem" a cloud.**

| Módulo | Linhas | Por quê |
|---|---|---|
| `invite.rs` | 22 | tipo nomeado pelo trait `Store` (R2, correção C1) |
| `sso.rs` | 84 | idem (R2, correção C1) |
| `tenant.rs` | 184 | `TenantId` e `DEFAULT_TENANT` atravessam tudo (R1, R2) |
| `domain.rs` | 39 | tipo `Domain` nomeado pelo trait (R2) |
| `domain_router.rs` | 931 | resolução de `Host` no hot path (R1) |
| `dns.rs` | 87 | seam de TXT, usado pela verificação; barato e sem valor comercial isolado |
| `oidc.rs` | 1612 | runtime OIDC compartilhado pelos dois modos (R5) |

## 5. O trait `Store`: fica inteiro no core

Dos 115 métodos, 46 servem entidades que só existem de verdade em cloud:

| Entidade | Métodos |
|---|---|
| tenants | 6 |
| memberships e users | 8 |
| invites | 7 |
| oidc_configs | 7 |
| sso_email_domains | 7 |
| domains | 9 |

Mesmo assim o trait **não se divide**. É um trait só, implementado por
`LmdbStore` e por `PostgresStore`, e os dois são core. Separar exigiria quebrar
o trait em dois e fazer o core depender de um supertrait opcional, o que
espalharia genéricos por toda a superfície de handler para ganhar zero: dado
persistido não é o que se vende, e o schema já é público de qualquer forma.

Consequência prática: a edição Community carrega o schema multi-tenant completo
e não sabe administrá-lo. É exatamente o que a D2 da spec decidiu, agora com o
número na mão.

## 6. Variáveis de ambiente

Cloud-only (11 de 59): `QUARK_MULTI_TENANT`, `QUARK_TENANT_DOMAIN_SUFFIX` e os
nove `QUARK_KEYCLOAK_*` (`BASE_URL`, `ADMIN_CLIENT_ID`, `ADMIN_CLIENT_SECRET`,
`PANEL_URL`, `LOGIN_THEME`, `SMTP_HOST`, `SMTP_PORT`, `SMTP_USER`,
`SMTP_PASSWORD`, `SMTP_FROM`, `SMTP_STARTTLS`).

`QUARK_ADMIN_HOST` fica no core: restringir `/admin/*` a um host é
endurecimento útil para qualquer deploy, não só para cloud.

No build Community essas variáveis passam a ser ignoradas em silêncio. A
`docs/CONFIGURATION.md` precisa marcar cada uma como "Enterprise", senão o
operador configura e fica esperando efeito.

## 7. Decisões em aberto

**7.1. Domínio próprio: EE ou core?** Hoje o código diz EE (os cinco handlers
retornam 404 fora de cloud). Mas R4 empurra para o core: um self-hoster
querer apontar `links.empresa.com` para a instância dele é pedido banal, e o
`QUARK_PUBLIC_HOST` já cobre um domínio só. O que a tela de domínios agrega é
**vários** domínios e verificação por DNS, que é o que interessa a quem revende.

Três saídas: manter EE como o código já faz; mover para o core inteiro; ou
partir, deixando um domínio no core (que é o `QUARK_PUBLIC_HOST` de hoje) e
múltiplos domínios mais verificação como EE. A terceira é a mais honesta com a
regra, e é a mais trabalhosa. **Precisa da tua decisão.**

**7.2. O painel Community perde a tela de domínios?** Consequência direta de
7.1. Se domínios for EE, `web/src/routes/Domains.tsx` vai junto.

**7.3. `oidc_tenants` fica no `AppState` do core?** É cache de `OidcRuntime` por
tenant, alimentado pelo store (core). Deixá-lo no core evita `cfg` dentro de
`oidc_login.rs` e custa um campo inerte no build Community. A recomendação é
deixar no core; a alternativa é adiantar a Fase 3 agora.

## 8. Consequência para a spec

A seção 4 da spec precisa ser reescrita com esta tabela, a seção 5 ganha a nota
de que `EeState` carrega só `keycloak` e `keycloak_base_url`, e a seção 7 ganha
a decisão de domínios como pré-requisito da Fase 2.

Números que a spec ainda não tinha: 15 rotas de 49 vão para a EE, 2665 linhas
de servidor movem de pasta, o trait `Store` inteiro fica, e 11 variáveis de
ambiente viram Enterprise.

## Como reproduzir este levantamento

```bash
# rotas e handlers
grep -n "\.route(" -A6 src/api/router.rs

# handlers que ja sao cloud-only
grep -rn "multi_tenant" --include=*.rs src/api/ | grep -v tests

# tipos que o trait Store nomeia
grep -n "Invite\|SsoEmailDomain\|OidcConfig\|Domain" src/store/mod.rs

# variaveis de ambiente
grep -rhoE 'QUARK_[A-Z_]+' src/ docs/CONFIGURATION.md | sort -u
```
