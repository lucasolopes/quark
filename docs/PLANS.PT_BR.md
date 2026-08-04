[English](PLANS.md) · **Português**

# Planos e entitlement

O quark Cloud limita um pequeno número de features e cotas por plano. Isso é
uma preocupação Enterprise: uma instalação Community self-hosted nunca tem
plano e nunca esbarra num limite. Se você roda o build AGPL para si mesmo,
esta página não se aplica a você; veja a nota abaixo.

## A edição Community não aplica limite nenhum

`src/api/entitlement.rs` é o seam por onde todo caminho de código limitado
passa. Quando o quark é buildado sem `--features ee`, as duas funções atrás
desse seam (`require`, `require_quota`) são stubs Community que sempre
retornam `Ok`. Não existe consulta de plano, não existe ida ao store, e não
tem como configurar um limite dentro do build Community. Isso é um invariante
de design, não um padrão que por acaso é generoso hoje: limitar uma
instalação self-hosted contradiria a separação open-core descrita em
[`specs/2026-08-03-luc19-open-core-design.md`](specs/2026-08-03-luc19-open-core-design.md).

Tudo abaixo descreve o build Enterprise (`--features ee`), que é o que o
quark Cloud roda.

## A grade de planos

Cinco planos, de `Free` a `Custom`, vivem em `crate::ee::plan::Plan`. Os
números são código, não configuração: mudar um limite é um deploy, e vale
para todo tenant naquele plano de uma vez.

| | Free | Starter | Pro | Business | Custom |
|---|---|---|---|---|---|
| Domínios | 3 | 10 | 50 | ilimitado | ilimitado¹ |
| Membros | 1 | 3 | 10 | ilimitado | ilimitado¹ |
| Execuções de automação / mês | 100 | 5.000 | 50.000 | 500.000 | ilimitado¹ |
| Cliques rastreados / mês | 50.000 | 250.000 | 1.000.000 | 5.000.000 | ilimitado¹ |
| Retenção de analytics | 30 dias | 365 dias | 730 dias | 1.095 dias | ilimitado¹ |

¹ `Custom` é o tier negociado para um cliente com contrato. Sem override por
tenant, é ilimitado em tudo. Uma coluna de override por tenant (`plan_limits`)
está desenhada mas ainda não construída; não tem consumidor até que um
cliente precise mesmo de um limite Custom mais estreito que "tudo".

O teto de membros é aplicado quando alguém resgata um convite via
`POST /admin/invites/:token/accept` (modelo A, sem IdP provisionado para o
tenant). Para um tenant com IdP próprio provisionado (Keycloak/modelo B), a
membership é concedida no primeiro login, a partir do group claim, e esse
caminho ainda não aplica a cota de membros. Um tenant nesse modelo pode
estourar o teto de membros bastando ter usuários distintos suficientes
fazendo login. É um furo conhecido, aceito nesta fase, registrado como
LUC-148; fechá-lo depende do contexto de billing que a fase 2 traz.

### Features (binário, não é teto)

| | Free | Starter | Pro | Business | Custom |
|---|---|---|---|---|---|
| Webhooks | – | ✓ | ✓ | ✓ | ✓ |
| Integrações (Sheets, pixels) | – | ✓ | ✓ | ✓ | ✓ |
| Monitoramento de link quebrado* | – | – | ✓ | ✓ | ✓ |
| Tokens de API com escopo* | – | – | ✓ | ✓ | ✓ |
| SSO | – | – | – | ✓ | ✓ |

\* Não aplicado nesta fase. `Feature` (`src/api/entitlement.rs`) ainda não tem
variante `HealthMonitoring` nem `TokenScopes`, nenhum handler checa nenhuma
das duas, e `GET /admin/plan` não pode listá-las como liberadas porque elas
não existem no código. Essas duas linhas descrevem o roadmap comercial, não o
comportamento atual; elas entram no código junto com a fatia de trabalho que
as ligar a um handler de verdade.

O Slack (`src/api/slack.rs`) não tem gate de plano nenhum e não faz parte da
grade comercial nesta fase; conectar é livre em qualquer plano, Free incluso.

Os contadores mensais (execuções de automação, cliques rastreados) estão
desenhados mas ainda não são aplicados; compartilham uma máquina de contagem
mensal que a fase 3 constrói. Hoje só as cotas por contagem de linha
(domínios, membros) e as features Webhooks, Integrações e SSO são de fato
limitadas. Monitoramento de link quebrado e tokens de API com escopo não são,
conforme a nota acima.

## Como é uma recusa

Uma feature ou cota que o plano não libera responde `402 Payment Required`,
nunca `403`: quem chamou está autorizado, o que falta é plano. O corpo diz o
que foi atingido e para onde ir:

```json
{
  "error": "plan_limit_reached",
  "limit": "webhooks",
  "allowed": null,
  "upgrade_to": "starter"
}
```

`allowed` é o teto atingido para uma cota (por exemplo `3` para domínios), ou
`null` para uma feature binária. `upgrade_to` é o plano mais barato que
libera o limite, calculado da mesma grade acima, então o painel nunca precisa
adivinhar nem manter cópia própria.

## O que o caminho de redirect nunca faz

`Plan` e o seam de entitlement nunca são consultados no caminho quente de
redirect (`src/api/links.rs`, `src/domain_router.rs`, `src/cache/mod.rs`).
Uma checagem de plano ali acrescentaria uma ida ao store ou ao cache na
requisição de maior tráfego do sistema, por uma decisão que só importa em
escrita. A aplicação acontece do lado admin/escrita: criar domínio, criar
webhook, convidar membro, e assim por diante.

## Lendo o plano atual

`GET /admin/plan` retorna o plano do tenant de quem chamou, seus tetos e as
features que libera:

```json
{
  "plan": "starter",
  "limits": {
    "domains": 10,
    "members": 3,
    "automation_per_month": 5000,
    "tracked_clicks_per_month": 250000,
    "retention_days": 365
  },
  "features": ["webhooks", "integrations"]
}
```

Um limite `null` significa ilimitado. O painel admin renderiza sua tela de
plano/uso a partir desse endpoint em vez de carregar cópia própria da grade,
que ficaria desatualizada assim que um limite mudasse.

Qualquer credencial que passe por `admin_guard` com `Scope::LinksRead` ou
maior pode ler isso. Não é sensível, e todo tenant já sabe o próprio plano
pelo produto que experimenta.

## Trocando o plano de um tenant

Ainda não existe gateway de pagamento (isso é a fase 2). Até lá, trocar o
plano de um tenant é ação do operador:

```
PUT /admin/tenants/{id}/plan
x-admin-token: <QUARK_ADMIN_TOKEN>
Content-Type: application/json

{ "plan": "starter" }
```

Duas coisas tornam esse endpoint diferente de toda outra rota admin:

- **Exige o break-glass `QUARK_ADMIN_TOKEN` diretamente**, comparado em
  tempo constante, e nada mais: nem token de API de tenant, nem sessão, nem
  qualquer credencial que `admin_guard` resolveria em nome de um tenant.
  Isso é proposital: `Plan::Custom` concede tudo ilimitado, e `"custom"` é
  uma string que o parser reconhece como qualquer outro nome de plano. Se
  uma credencial de tenant pudesse escrever essa coluna, qualquer cliente
  poderia se promover a ilimitado. Só o operador que tem o token admin do
  deploy pode trocar um plano.
- **Uma string de plano desconhecida é rejeitada com `400`**, não aceita em
  silêncio. `Plan::from_stored` (usado em toda leitura) cai para `Free` numa
  string desconhecida, o que é a escolha segura para leitura: um valor
  corrompido no store não pode derrubar o produto nem conceder mais do que o
  pretendido. Numa escrita, esse mesmo fallback seria perigoso na direção
  oposta: um typo como `"starterr"` rebaixaria o tenant para Free em
  silêncio em vez de falhar alto. O handler de escrita compara a string
  canônica do plano já interpretado com o que foi enviado e rejeita qualquer
  coisa que não bata.

A troca vale de imediato no nó que atendeu esta requisição: o handler chama
`st.ee.plans.invalidate(tenant)` depois de gravar o plano, então a próxima
requisição NESSE nó não espera os 60 segundos do TTL do cache de plano.
`PlanCache` é por processo, sem invalidação entre nós, então num deploy com
várias réplicas os outros nós continuam respondendo com o próprio cache até
convergir sozinhos, dentro do mesmo TTL de 60 segundos. Ligar a invalidação
entre nós (o canal pub/sub que `src/invalidate.rs` já usa para entradas de
cache) é trabalho futuro.

## A página de preços é cópia, não fonte

A grade de planos que o site de marketing ou a página de preços mostra é
cópia, mantida sincronizada à mão. `crate::ee::plan::Plan` neste repositório
é a única fonte de verdade sobre o que um plano de fato aplica. Se as duas
discordarem algum dia, o código vence, e a página de preços precisa ser
corrigida.
