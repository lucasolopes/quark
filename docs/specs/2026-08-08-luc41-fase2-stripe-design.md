# Stripe: assinatura, portal e webhooks (LUC-41, Fase 2)

Design da segunda fatia do billing. Liga a cobrança de verdade: customer por
tenant, checkout hospedado, Customer Portal e webhooks idempotentes que
escrevem o plano. A aplicação dos limites já existe (Fase 1,
`docs/specs/2026-08-03-planos-e-entitlement-design.md`) e não muda aqui.

A grade e os preços vêm de `docs/DECISAO-planos-e-pricing-cloud.md` (LUC-64).
A pesquisa que fundamenta as decisões desta spec está em
`docs/research/2026-08-08-luc41-fase2-stripe-research.md`.

## 1. Escopo

Entra: SDK, colunas de billing no tenant, endpoint de checkout, endpoint de
portal, endpoint de webhook, mapeamento de estado de assinatura para plano,
docs e runbook de configuração do Stripe.

Não entra: front de pricing ou tela de billing no painel (espera a landing),
custom domain do checkout (espera a migração para quark.sh, LUC-147), soft cap
e contadores mensais (Fase 3), Stripe Tax (não suporta merchant sediado no
Brasil).

## 2. Decisões

### D1. SDK: async-stripe 1.0.0-rc, crates split, versão pinada

A superfície usada é pequena, mas manter um cliente HTTP próprio do Stripe
seria assumir suporte de uma implementação que uma lib mantida já entrega.
Decidido usar a linha 1.0.0-rc do async-stripe (github.com/arlyon/async-stripe),
que é regenerada semanalmente do OpenAPI do Stripe e publicada em crates por
área. Entram só os crates da superfície usada: cliente base, checkout, billing
(portal e subscriptions) e webhook.

A linha é pré-release, então a versão é pinada exata no `Cargo.toml` (rc.8 no
momento desta spec) e só sobe por decisão, não por `cargo update`.

Descartado: cliente próprio com reqwest (recomendação original da pesquisa).
Menos dependências, mas vira código nosso para sempre numa área onde erro
custa dinheiro. Descartado: linha 0.41 estável, que está congelada e presa a
uma versão antiga da API do Stripe.

### D2. Checkout e portal hospedados no Stripe, sem custom domain no lançamento

O usuário paga em `checkout.stripe.com` e gerencia em portal hospedado do
Stripe. Custom domain de checkout é feature paga (10 USD/mês) e a migração de
domínio pendente (quarkus.com.br para quark.sh, LUC-147) obrigaria a
configurar duas vezes, com risco de quebrar links de portal já emitidos ao
remover o domínio antigo. `checkout.quark.sh` fica para depois da migração,
se valer o custo.

### D3. Prices por lookup key, catálogo de preço fora do código e fora do env

Cada price no Stripe ganha `lookup_key` estável (`starter-monthly`,
`starter-yearly`, `pro-monthly`, e assim por diante). O backend resolve o
price por lookup key na hora do checkout, e inverte a lookup key da
subscription para descobrir o plano no webhook. As lookup keys são `const` em
`src/ee/stripe/`; os price IDs em si nunca aparecem em código nem em env, e
são os mesmos nomes em test mode e live mode.

São 6 prices: 3 planos autosserviço (Starter, Pro, Business) vezes 2 ciclos,
cada price com moeda USD e BRL (multi-currency price). Anual com dois meses
grátis, como a LUC-64 fixou. Custom fica fora do autosserviço: é negociado, e
o operador seta pelo escape hatch da Fase 1.

### D4. O plano só é escrito pelo webhook

Nenhum endpoint de checkout ou portal escreve `plan`. A única escrita
automática é o handler de webhook, e a manual é o escape hatch do operador da
Fase 1 (`PUT /admin/tenants/{id}/plan`). Isso mantém um caminho único de
verdade e torna o fluxo auditável.

O webhook é idempotente por event id: tabela `stripe_events` com insert
`ON CONFLICT DO NOTHING`; evento repetido responde 200 e não reaplica. Para
estado de assinatura, o handler nunca confia no payload do evento: busca a
subscription atual na API e deriva o plano dela, porque a ordem de entrega de
eventos não é garantida. Retry de falha nossa é do Stripe (resposta 5xx), sem
outbox próprio.

### D5. Moeda é decisão de primeiro checkout, por tenant

Com multi-currency prices o Stripe escolheria a moeda pelo IP, mas a moeda do
customer trava na primeira assinatura. Então o corpo do checkout carrega
`currency` (`usd` ou `brl`), o backend força na session, e a escolha vale para
a vida do customer. Trocar de moeda depois é operação manual (novo customer),
fora de escopo.

### D6. Trial de 14 dias sem cartão

`subscription_data.trial_period_days = 14` com `trial_settings` permitindo
checkout sem método de pagamento. Fim do trial sem cartão cadastrado cancela a
subscription, o webhook rebaixa para Free. Estado `trialing` conta como plano
pago no mapeamento (D8).

### D7. Billing é do Owner

Iniciar checkout e abrir o portal exigem `Role::Owner` no tenant, o mesmo
padrão de excluir workspace. Admin gerencia o produto, não o dinheiro.

### D8. Dunning: aguentar `past_due`, rebaixar nos terminais

Smart Retries ligado no dashboard (8 tentativas em duas semanas). Mapeamento
de status de subscription para plano efetivo, numa função pura:

| Status | Plano efetivo |
|---|---|
| `active`, `trialing`, `past_due` | o plano do price da subscription |
| `canceled`, `unpaid`, `incomplete_expired`, `paused` | `free` |

`past_due` mantém o acesso durante a janela de retry; e-mails de recuperação
são os automáticos do Stripe, configurados no dashboard. Downgrade com
recursos acima do teto do plano novo não apaga nada: os gates da Fase 1 já
bloqueiam criação nova, o que existe continua funcionando.

### D9. Billing é opcional por env, como o Keycloak

`QUARK_STRIPE_SECRET_KEY` e `QUARK_STRIPE_WEBHOOK_SECRET` configurados ligam o
billing; sem eles, `EeState.billing` é `None` e os endpoints respondem 404. Um
self-host Enterprise sem Stripe continua funcionando por inteiro, que é a
razão de a camada de plano ser independente do gateway desde a Fase 1.

## 3. Modelo de dados

Migrações no `init_schema`, padrão da Fase 1:

```sql
ALTER TABLE tenants ADD COLUMN IF NOT EXISTS stripe_customer_id TEXT;
ALTER TABLE tenants ADD COLUMN IF NOT EXISTS stripe_subscription_id TEXT;
CREATE TABLE IF NOT EXISTS stripe_events (
    id TEXT PRIMARY KEY,
    type TEXT NOT NULL,
    received_at BIGINT NOT NULL
);
```

Métodos novos no trait `Store`, strings opacas como `plan`:
`get_stripe_customer_id`, `set_stripe_customer_id`,
`set_stripe_subscription_id`, `find_tenant_by_stripe_customer` (o webhook
chega com customer id) e `record_stripe_event(id, type) -> bool` (false se
duplicado). LMDB implementa tudo inerte: billing é cloud-only.

## 4. Endpoints

### `POST /admin/billing/checkout`

Owner-only. Body: `{ "plan": "starter"|"pro"|"business", "cycle":
"monthly"|"yearly", "currency": "usd"|"brl" }`. Cria o customer no Stripe se o
tenant não tem (guarda o id), cria Checkout Session em modo subscription com o
price resolvido por lookup key, `client_reference_id` com o tenant id,
metadata com o tenant id na subscription, trial conforme D6 e currency
conforme D5. Responde `{ "url": ... }`; o caller redireciona. `success_url` e
`cancel_url` apontam para o painel.

### `POST /admin/billing/portal`

Owner-only. Exige customer existente (404 sem ele). Cria uma sessão do
Customer Portal e responde `{ "url": ... }`. Upgrade, downgrade, cancelamento,
método de pagamento e faturas acontecem no portal, cuja configuração (feita
uma vez no dashboard, documentada no runbook) restringe a troca aos nossos
products e desliga alteração de quantidade. Toda mudança volta via webhook.

### `POST /stripe/webhook`

Público, sem `admin_guard`; a autenticação é a assinatura `Stripe-Signature`
(HMAC-SHA256, tolerância de 5 minutos, verificada pelo crate de webhook).
Fluxo: assinatura inválida responde 400; evento duplicado responde 200 sem
efeito; falha nossa responde 5xx e o Stripe retenta.

| Evento | Ação |
|---|---|
| `checkout.session.completed` | grava `stripe_subscription_id` no tenant |
| `customer.subscription.created` / `updated` / `deleted` | busca a subscription na API, mapeia status e lookup key para plano (D8), `set_tenant_plan` e invalida o `PlanCache` |
| `invoice.paid` | log estruturado |
| `invoice.payment_failed` | log estruturado |

Customer sem tenant correspondente: warning e 200 (evento órfão não fica em
retry eterno).

## 5. Erros

Stripe indisponível no checkout ou portal responde 503. Nada de billing toca o
caminho de redirect, que segue a regra absoluta da Fase 1. O webhook responde
rápido e deixa o retry com o Stripe.

## 6. Testes

- Unit: a função de mapeamento status + lookup key para plano, tabela
  completa; resolução de lookup key para plano e vice-versa.
- Integração (`tests/billing_it.rs`, `#![cfg(feature = "ee")]`, Postgres
  gated): webhook com payload assinado com secret de teste muda o plano no
  banco e invalida o cache; evento duplicado não reaplica; assinatura inválida
  responde 400; billing desligado responde 404 nos endpoints. Tudo offline,
  sem conta Stripe, roda no CI.
- Sandbox manual (runbook, fora do CI): `stripe listen` no dev local; test
  clocks para renovação e dunning ponta a ponta.

## 7. Documentação

`docs/BILLING.md` e twin PT_BR: fluxo do usuário, estados de assinatura, o
que muda no plano e quando. `docs/RUNBOOK-stripe.md`: setup único do
dashboard (products e prices com lookup keys, config do portal, endpoint de
webhook, secrets, Smart Retries e e-mails de recuperação), e a nota fiscal:
sem Stripe Tax, NFS-e e ISS são processo fora do Stripe.

## 8. Em aberto

- Os preços finais em USD e BRL viram compromisso na página de preços, não
  aqui. Considerar as taxas de venda internacional (na faixa de 2,5% extras
  cobrando USD numa conta brasileira) ao fechar os pontos.
- Nome do quarto degrau segue provisório ("Business", herdado da LUC-64).
- Quando a migração para quark.sh acontecer, reavaliar o custom domain de
  checkout.
- Subir o pin do async-stripe quando o 1.0 estável sair.
