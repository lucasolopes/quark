[English](LICENSING.md) · **Português**

# Licenciamento: Community e Enterprise

O quark é open core. O núcleo é software livre sob AGPL. Um conjunto pequeno e
bem delimitado de diretórios é proprietário e cobre o que só importa para quem
opera o quark como serviço para outras pessoas.

## A versão curta

| | Community | Enterprise |
|---|---|---|
| Licença | AGPL-3.0-only | quark Enterprise Edition License |
| Onde | tudo, menos os dois caminhos abaixo | `src/ee/`, `web/src/ee/` |
| Custo | grátis, sem limite | assinatura comercial |
| Build | `cargo build` | `cargo build --features ee` |
| Para quem | uma organização rodando o quark para si mesma | operar o quark como serviço para terceiros |

Apagar `src/ee/` e `web/src/ee/` deixa um quark completo, que compila e é
inteiramente AGPL. O CI prova isso a cada push, então não tem como deixar de ser
verdade em silêncio.

## O que tem em cada edição

**A Community tem o produto inteiro para um workspace**: o caminho de redirect,
código curto customizado, variantes A/B, regras por dispositivo e geografia,
deep links, senha de link, expiração, analytics, webhooks, pixels, Google
Sheets, Slack, tokens de API, importação em massa, monitoramento de link
quebrado, o painel administrativo e login com o seu próprio provedor de
identidade via OIDC.

**A Enterprise acrescenta o que um operador precisa para rodar o quark para
outras pessoas**: criar e excluir workspaces, convidar membros, configurar
provedor de identidade por tenant, descoberta de SSO por domínio de e-mail,
provisionamento automático de realm no Keycloak e múltiplos domínios próprios
com verificação por DNS. Billing e limites de plano também nascem aqui.

A linha não é "qual o tamanho da sua empresa". É se as contas que você
administra são suas ou de terceiros.

## Por que o núcleo é AGPL

A cláusula 13 da AGPL diz que, se você roda um quark modificado como serviço de
rede, quem usa esse serviço tem direito às suas modificações. É a proteção que o
projeto quer, e ela vale para nós também: o serviço hospedado do quark roda o
mesmo núcleo publicado aqui.

Licenças comerciais do núcleo, para usá-lo sem as obrigações de copyleft da
AGPL, estão disponíveis sob consulta.

## Usando o código Enterprise

Os diretórios `src/ee/` e `web/src/ee/` são publicados como source-available,
não escondidos. Você pode ler, auditar, compilar e desenvolver em cima deles.
Rodar em produção exige assinatura Enterprise válida. Os termos exatos estão em
`src/ee/LICENSE` e `web/src/ee/LICENSE`.

Publicar esse código é proposital: ninguém deveria ter que confiar os próprios
links a uma caixa preta, e a parte pela qual cobramos tem que ser tão
inspecionável quanto a parte gratuita.

## Como buildar cada edição

```bash
# Community: o padrão. Não contém nada de código Enterprise.
cargo build --release
cd web && npm run build

# Enterprise
cargo build --release --features ee
cd web && VITE_QUARK_EE=1 npm run build
```

A imagem de container publicada é a Community.

Os testes seguem a mesma divisão: `cargo test` e `npm run test` cobrem a
Community, `cargo test --features ee` e `npm run test:ee` somam a superfície
Enterprise.

## Variáveis de ambiente que só têm efeito na Enterprise

`QUARK_MULTI_TENANT`, `QUARK_TENANT_DOMAIN_SUFFIX` e todas as
`QUARK_KEYCLOAK_*`. Um build Community as ignora. Ver
[`CONFIGURATION.PT_BR.md`](CONFIGURATION.PT_BR.md), onde cada uma está marcada.

## Contribuindo

Contribuição é bem-vinda nas duas árvores. Um pull request que toca `src/ee/` ou
`web/src/ee/` entra sob a licença Enterprise daquele diretório, e não sob a
AGPL; todo o resto é AGPL como sempre. Nos dois casos você continua dono da sua
contribuição, e o [CLA](../CLA.PT_BR.md) detalha a cessão.

## O raciocínio por trás do corte

O documento de design, com os benchmarks contra n8n, Chatwoot, PostHog, Cal.com,
Dub, Plausible, GitLab e OpenObserve, está em
[`specs/2026-08-03-luc19-open-core-design.md`](specs/2026-08-03-luc19-open-core-design.md).
O inventário arquivo a arquivo que decidiu onde cada rota e módulo caiu está em
[`research/2026-08-03-luc19-inventario-oss-ee.md`](research/2026-08-03-luc19-inventario-oss-ee.md).
