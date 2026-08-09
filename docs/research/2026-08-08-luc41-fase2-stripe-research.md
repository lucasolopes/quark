# LUC-41 Fase 2: pesquisa de integração Stripe (billing do quark cloud)

Data: 2026-08-08. Todas as fontes foram verificadas nesta data.

Escopo: subsidiar a spec da Fase 2 do billing. Decisões já tomadas e fora de
discussão: assinatura pura de plano (Free/Starter/Pro/Business/Custom), sem
quantidade, sem uso reportado ao Stripe, sem proração de assento; preços USD e
BRL fixados à mão como multi-currency prices no mesmo produto; catálogo de
limites em código Rust; Stripe nunca é fonte da verdade de entitlement; a
Entitlements API do Stripe foi descartada.

Referência de implementação usada como benchmark: o repositório open source do
Dub (github.com/dubinc/dub), que vende exatamente o mesmo tipo de produto
(encurtador SaaS com planos por workspace) sobre Stripe.

## 1. Checkout hospedado em domínio próprio

Resposta direta: sim, o Stripe Checkout suporta domínio próprio, mas é feature
paga (10 USD/mês) e só aceita subdomínio. Para o lançamento, não vale a pena:
`checkout.stripe.com` custa zero, é o padrão do mercado, e a migração
quarkus.com.br para quark.sh obrigaria a fazer a configuração de DNS e a
verificação duas vezes.

Detalhes:

- Um único custom domain por conta cobre as três superfícies hospedadas:
  Checkout, Payment Links e customer portal. Custo de 10 USD/mês, cobrado no
  início do mês seguinte, e só em meses em que o domínio ficou habilitado o mês
  inteiro. Fonte: https://support.stripe.com/questions/custom-domain-on-stripe-checkout-faq
  e https://docs.stripe.com/payments/checkout/custom-domains
- Configuração: escolher um subdomínio (ex.: `pay.quark.sh`; caminho tipo
  `quark.sh/checkout` não é aceito), criar um CNAME apontando para
  `hosted-checkout.stripecdn.com` e um TXT `_acme-challenge.<sub>` para o
  desafio ACME. O Stripe emite TLS via Let's Encrypt (atenção a registros CAA:
  precisam permitir `letsencrypt.org`). Ativação automática após verificação
  do DNS.
- Limitações relevantes: só funciona com o fluxo de redirect server-side
  (criar a Session no backend e redirecionar para `session.url`, que é o fluxo
  que vamos usar de qualquer forma); não funciona em sandbox; um domínio por
  conta.
- Impacto da migração de marca: remover o domínio exige apagar os registros
  DNS e refazer o guia inteiro no domínio novo. Ou seja, configurar em
  quarkus.com.br agora significa pagar duas vezes o trabalho (e possivelmente
  um período de troca com links antigos quebrando: ao remover o domínio,
  links de portal e payment links no domínio antigo param de funcionar).
- Recomendação: lançar com `checkout.stripe.com` e `billing.stripe.com`
  (portal). Reavaliar o custom domain uma única vez, depois da migração para
  quark.sh, se houver motivo de marca ou conversão. Nada no backend muda: o
  redirect usa sempre a URL retornada pela API.

## 2. Checkout Sessions vs Payment Links

Resposta direta: para upgrade self-service iniciado por tenant autenticado, o
fluxo recomendado é Checkout Session criada no backend (`mode=subscription`)
com redirect para `session.url`. Payment Links são links estáticos para
compartilhar sem backend; não servem para amarrar a sessão ao tenant nem para
fixar `customer`, então não se aplicam aqui.

Parâmetros relevantes (fonte:
https://docs.stripe.com/api/checkout/sessions/create):

- `mode=subscription`, com `line_items[0][price]` apontando para o price do
  plano e `quantity=1` (sem assento, quantidade sempre 1).
- `customer`: passar o Stripe customer do tenant (criado lazy no primeiro
  upgrade e persistido no banco). Evita duplicar customers e garante que o
  webhook consiga resolver o tenant pelo `customer`.
- `client_reference_id`: id do tenant (até 200 chars). É o campo que o Dub usa
  para amarrar o checkout ao workspace no `checkout.session.completed`
  (verificado em `apps/web/app/(ee)/api/stripe/webhook/checkout-session-completed.ts`).
  Usar os dois cintos: `client_reference_id` E `customer`.
- `metadata` (na session) e `subscription_data[metadata]` (propagada para a
  subscription criada): gravar `tenant_id` e o nome do plano interno também na
  subscription, porque os eventos `customer.subscription.*` carregam a
  subscription, não a session.
- `success_url` (o Stripe acrescenta `session_id` como query param se você
  incluir o placeholder `{CHECKOUT_SESSION_ID}`) e `cancel_url` apontando de
  volta para o painel.
- `locale`: aceita `pt-BR`; `auto` segue o browser. Passar o locale do painel.
- `currency`: com multi-currency prices, o Checkout escolhe a moeda pelo IP do
  cliente quando o price tem `currency_options` para a moeda local, senão cai
  na moeda default do price. O parâmetro `currency` da session força
  explicitamente qual `currency_option` usar. Fontes:
  https://docs.stripe.com/payments/checkout/localize-prices/manual-currency-prices
  e https://docs.stripe.com/products-prices/manage-prices
- Restrição importante de moeda: todos os prices de uma session precisam ter a
  mesma moeda default, e o objeto Customer fica travado em uma única moeda de
  faturamento a partir da primeira subscription (`customer.currency`). Troca
  de plano depois disso tem que continuar na mesma moeda. Recomendação: deixar
  o backend decidir a moeda (preferência do tenant ou moeda já gravada no
  customer) e passar `currency` explícito, em vez de confiar no IP.

## 3. Customer Portal

Resposta direta: o portal (billing.stripe.com) resolve de graça quase todo o
self-service: troca de plano entre products permitidos, cancelamento (imediato
ou no fim do período), atualização de método de pagamento e histórico de
faturas. Configuração no Dashboard (ou via API de portal configuration), e a
sessão é criada no backend com um POST em `/v1/billing_portal/sessions`.

Fontes: https://docs.stripe.com/customer-management e
https://docs.stripe.com/customer-management/configure-portal

- Restringir o que aparece: a opção "Switch plan" da configuração lista
  explicitamente quais products/prices o cliente pode escolher (máximo de 10
  products). Colocar só Starter/Pro/Business nas duas moedas; Custom fica fora
  e é tratado pelo operador.
- Sem assento: deixar "Update quantities" desligado (é o default). Com isso o
  portal nunca oferece mudança de quantidade e o requisito "sem cobrança por
  assento" está atendido sem código.
- Proração: a configuração tem "Prorate subscription updates" (crédito do
  tempo restante ao trocar de plano, aplicado imediatamente ou no fim do
  período) e "Manage downgrades" (agendar o downgrade para o fim do período
  via subscription schedule; só entre prices do mesmo product). Para
  comportamento padrão do Stripe: upgrades imediatos com proração de tempo
  (não de assento, que não existe no nosso modelo) e, se quisermos, downgrade
  no fim do período.
- Cancelamento: on por default, com opção de coletar motivo e cupom de
  retenção. Cancelar "at period end" mantém acesso até o fim do ciclo (o
  webhook `customer.subscription.updated` chega com `cancel_at_period_end`).
- Custom domain: o mesmo custom domain pago do item 1 cobre o portal (um por
  conta). Sem ele, o portal vive em `billing.stripe.com`.
- Limitações que nos afetam pouco: portal não atualiza assinaturas com
  múltiplos products ou usage-based (não é o nosso caso); a troca exige
  `tax_behavior` consistente entre prices; portal não roda em iframe.
- Localização: pt-BR suportado automaticamente pelo idioma do browser.

## 4. Webhooks

Resposta direta: a lista mínima para manter o plano do tenant sincronizado é
`checkout.session.completed`, `customer.subscription.created`,
`customer.subscription.updated`, `customer.subscription.deleted`,
`invoice.paid` e `invoice.payment_failed`. Verificar a assinatura
`Stripe-Signature` (HMAC-SHA256, tolerância default de 5 minutos), deduplicar
por `event.id`, não depender de ordem, e tratar o payload como gatilho: buscar
o estado atual da subscription na API antes de gravar o plano.

Fontes: https://docs.stripe.com/billing/subscriptions/webhooks e
https://docs.stripe.com/webhooks

- Eventos, confirmados na doc atual de assinaturas:
  - `checkout.session.completed`: amarra customer ao tenant e provisiona o
    plano no primeiro upgrade.
  - `customer.subscription.created/updated/deleted`: qualquer mudança de
    plano, renovação, cancelamento, mudança de status.
  - `invoice.paid`: a doc diz explicitamente "provisione o acesso quando
    receber este evento e o status for active".
  - `invoice.payment_failed`: falha de cobrança (ver seção 5).
  - Opcional: `customer.subscription.trial_will_end` se um dia houver trial.
- Assinatura: header `Stripe-Signature` no formato `t=<ts>,v1=<hmac>`; HMAC
  SHA-256 do string `"{t}.{corpo_bruto}"` com o signing secret do endpoint;
  ignorar schemes diferentes de `v1`; comparação em tempo constante;
  tolerância default das libs oficiais é 5 minutos e a doc manda nunca usar
  tolerância 0. O corpo tem que ser o raw body, sem reparse.
- Idempotência: o mesmo evento pode chegar mais de uma vez; a doc recomenda
  registrar os `event.id` processados e ignorar repetidos. Em alguns casos o
  Stripe gera dois objetos Event distintos para a mesma mudança, então o dedup
  robusto usa `(data.object.id, event.type)` além do `event.id`. Retentativas
  automáticas por até 3 dias com backoff exponencial em produção.
- Ordem: não garantida (a doc dá exatamente o exemplo de criação de
  subscription gerando `customer.subscription.created`, `invoice.created`,
  `invoice.paid`, `charge.created` em ordem qualquer). Padrão recomendado:
  responder 2xx rápido, processar async, e "fetch the latest state": usar o
  evento só como sinal e buscar `GET /v1/subscriptions/{id}` para decidir o
  plano efetivo. Isso torna o handler naturalmente convergente: eventos fora
  de ordem sempre terminam no estado atual.
- Como o Dub faz (verificado em
  `apps/web/app/(ee)/api/stripe/webhook/` no branch main em 2026-08-08):
  um único `route.ts` valida a assinatura com
  `stripe.webhooks.constructEvent(rawBody, sig, secret)`, tem um set de
  eventos relevantes e um switch que despacha para um arquivo por evento:
  `checkout-session-completed.ts`, `customer-subscription-created.ts`,
  `customer-subscription-updated.ts`, `customer-subscription-deleted.ts`,
  `invoice-payment-failed.tsx`, mais eventos de charge/transfer que são do
  produto de payouts deles, não do billing de plano. No
  `checkout.session.completed` eles resolvem o workspace por
  `client_reference_id`, gravam `stripeId` (customer) no workspace, fazem
  `stripe.subscriptions.retrieve(...)` (fetch latest state) e mapeiam
  `items.data[0].price.id` para o plano com uma tabela em código. No
  `customer-subscription-updated` resolvem o workspace pelo `stripeId` e
  mapeiam o price id do payload para o plano. Curiosidade: o route.ts do Dub
  NÃO deduplica por event id; eles confiam na idempotência natural dos
  handlers (upsert do plano). Para o quark, com o dedup por event id numa
  tabela, ficamos acima do benchmark.
- Mapeamento price -> plano: manter em código Rust (tabela price_id por moeda
  por plano), que já é a decisão do catálogo. É exatamente o que o Dub faz
  (`getPlanAndTierFromPriceId`).

## 5. Dunning / falha de cobrança

Resposta direta: com Smart Retries ligado (default recomendado: 8 tentativas
em 2 semanas), uma renovação que falha deixa a subscription `past_due` e o
Stripe retenta sozinho. Esgotadas as tentativas, a configuração do Dashboard
decide o destino: `canceled`, `unpaid` ou permanecer `past_due`. O mapeamento
recomendado para o plano efetivo: `active` e `trialing` mantêm o plano pago;
`past_due` mantém o plano pago (período de graça, avisar o tenant); `unpaid`,
`canceled`, `incomplete_expired` e `paused` rebaixam para Free.

Fontes: https://docs.stripe.com/billing/revenue-recovery/smart-retries e
https://docs.stripe.com/billing/subscriptions/webhooks (tabela de status)

- Smart Retries usa modelo de ML para escolher os horários; configurável em
  Billing > Revenue recovery > Retries (número de tentativas e janela: 1
  semana a 2 meses; default recomendado 8 tentativas em 2 semanas). Hard
  declines (cartão roubado, etc.) não são retentados sem novo método de
  pagamento.
- A própria doc de webhooks manda: quando mudar para `past_due`, notificar o
  cliente e pedir atualização do pagamento; quando mudar para `canceled` ou
  `unpaid`, revogar o acesso. É literalmente o mapeamento acima.
- Configuração sugerida no Dashboard: Smart Retries on, e ao esgotar,
  "cancel the subscription" (estado terminal limpo; o webhook
  `customer.subscription.deleted` rebaixa para Free). A alternativa `unpaid`
  mantém a subscription viva gerando faturas em rascunho, o que só vale se
  quisermos reativação pagando a última fatura sem novo checkout.
- Implementação no quark: como o plano efetivo deriva do status da
  subscription buscada na API, o dunning inteiro vira um `match status` num
  único ponto. `invoice.payment_failed` serve para notificar o tenant (e o
  Stripe também tem e-mails automáticos de recuperação configuráveis).

## 6. SDK Rust

Resposta direta: o ecossistema vivo é o `async-stripe` (github.com/arlyon/
async-stripe, começou como fork do antigo `stripe-rs`), ativamente mantido com
regeneração semanal a partir do OpenAPI do Stripe. Mas a recomendação para o
quark é NÃO adicionar o SDK: chamar a REST API direto com o `reqwest` que o
repo já usa, e verificar a assinatura do webhook com os `hmac` + `sha2` que o
repo já tem. Zero crates novos.

Detalhes:

- Estado do async-stripe (verificado no repositório em 2026-08-08): mantido,
  CI semanal regenerando o código da spec; passou por uma reescrita v1 que
  quebrou o monólito em crates por área (`stripe-checkout`, `stripe-billing`,
  `stripe-webhook` / publicados no crates.io sob nomes `async-stripe-*`,
  ex. `async-stripe-webhook`), justamente para reduzir tempo de compilação.
  A linhagem monolítica antiga está em 0.41.x no crates.io. Cobertura:
  Checkout Sessions (crate checkout), Billing Portal (crate billing) e
  verificação de assinatura de webhook (crate webhook, que inclui construtor
  com timestamp manual para testes). Não verifiquei se a série v1 já saiu de
  pré-release estável; o próprio split em ~vinte crates gerados é o problema
  para a política de dependências do quark.
  Fontes: https://github.com/arlyon/async-stripe,
  https://crates.io/crates/async-stripe,
  https://crates.io/crates/async-stripe-webhook
- Por que reqwest direto ganha aqui: a superfície que a Fase 2 usa é mínima e
  estável, algo como 5 chamadas (criar customer, criar checkout session,
  criar portal session, buscar subscription, talvez listar/atualizar
  customer). Todas são POST/GET form-encoded com bearer token. O repo já tem
  `reqwest` com `connect_timeout` padronizado em todos os clientes, `serde`
  para as structs de resposta (deserializar só os campos que usamos), e
  `hmac`/`sha2` para a verificação `v1` do `Stripe-Signature` (~30 linhas,
  algoritmo documentado publicamente, comparação em tempo constante via
  `hmac::Mac::verify_slice`). Um SDK gerado inteiro para isso não passa no
  crivo da skill quark-rust de justificar cada crate.
- Ponto de atenção do caminho manual: fixar a versão da API via header
  `Stripe-Version` nas chamadas e na configuração do endpoint de webhook, para
  o shape dos JSONs não mudar embaixo de nós.

## 7. Vendedor Brasil/global

Resposta direta: dá para lançar com uma conta Stripe Brasil vendendo em USD e
BRL sem Stripe Tax. Stripe Tax não suporta negócio sediado no Brasil coletando
imposto doméstico (na América Latina, só México como país-sede), então imposto
brasileiro (ISS, nota fiscal) é obrigação do merchant fora do Stripe de
qualquer jeito. IOF é majoritariamente problema do comprador, não do merchant.

Detalhes e mapa do que existe:

- Conta Stripe Brasil: taxa doméstica de cartão ~3,99% + R$0,39, com
  +1,5% para cartão emitido fora do país e +1% de conversão quando a moeda do
  pagamento difere da moeda de liquidação (BRL). Ou seja, cobrar USD numa
  conta BR embute conversão; os preços USD fixados à mão devem considerar essa
  margem. Fontes: https://stripe.com/pricing (via
  https://checkoutpage.com/blog/stripe-international-fees) e
  https://support.stripe.com/questions/taxes-on-stripe-fees-for-brazil-based-businesses
  (impostos indiretos brasileiros já inclusos nas fees do Stripe).
- Stripe Tax: a página de países suportados da América Latina diz que o
  Stripe só coleta imposto com negócio sediado no México; para os demais
  países listados, só como remote seller vendendo para dentro deles (produtos
  digitais). Não há suporte a negócio sediado no Brasil coletando ISS/IVA
  doméstico. Conclusão: Stripe Tax NÃO é necessário nem utilizável no
  lançamento para a obrigação doméstica; emissão de NFS-e e recolhimento são
  processo próprio (fora do escopo desta pesquisa). Fonte:
  https://docs.stripe.com/tax/supported-countries/latin-america-and-caribbean
- Vendas para fora (USD): exportação de serviço; o Stripe Tax poderia
  monitorar thresholds de registro em outros países (ex.: VAT europeu sobre
  serviços digitais B2C) quando houver volume; no lançamento, com volume
  pequeno, é aceitável lançar sem e monitorar. Isso é uma leitura prática, não
  parecer jurídico.
- IOF: incide sobre o comprador brasileiro que paga em moeda estrangeira
  (cartão internacional, ~3,5% atual) e sobre operações de câmbio do merchant
  no repatriamento. Como teremos price BRL nativo para clientes brasileiros
  (multi-currency), cliente BR paga em BRL e não sofre IOF de cartão; o IOF
  vira questão só do fluxo cambial da empresa. Fonte:
  https://www.pagbrasil.com/blog/pix/understanding-iof-tax-on-financial-operations-on-international-pix-payments/
- Alternativa mapeada, não recomendada agora: Managed Payments (merchant of
  record do Stripe) assume imposto global, mas muda a relação contratual e a
  precificação; overkill para o lançamento. Fonte:
  https://stripe.com/managed-payments
- Não verificado: se a entidade que vai assinar a conta Stripe será PJ
  brasileira mesmo, e o tratamento fiscal de exportação de SaaS; isso é
  decisão contábil/jurídica, não técnica.

## 8. Ambiente de teste

Resposta direta: sandbox isolado por padrão, `stripe listen --forward-to`
para webhooks locais, test clocks (Billing Simulations) para avançar o tempo
do ciclo de assinatura, e em CI dá para testar o endpoint de webhook sem conta
Stripe nenhuma: gerar o payload e assinar localmente com um `whsec_` de teste,
porque a assinatura é só HMAC-SHA256 de `"{timestamp}.{payload}"`.

Detalhes:

- Sandboxes: ambiente de teste isolado da conta; chaves e webhook secrets
  próprios. Custom domain não funciona em sandbox (irrelevante, ver item 1).
  Fonte: https://docs.stripe.com/sandboxes
- Stripe CLI: `stripe listen --forward-to localhost:PORT/path` encaminha
  eventos reais do sandbox para o endpoint local e imprime o
  `whsec_...` para verificação; `stripe trigger checkout.session.completed`
  dispara fixtures; `stripe events resend <id>` reenvia manualmente (até 30
  dias). Fonte: https://docs.stripe.com/webhooks (seção de teste)
- Test clocks / Billing Simulations: criam um relógio simulado, atrelam
  customers a ele e avançam o tempo para exercitar renovação, falha de
  pagamento em renovação, trial, upgrade no meio do ciclo, com os webhooks de
  ciclo de vida disparando de verdade. É o mecanismo para validar o fluxo de
  dunning da seção 5 sem esperar um mês. Fonte:
  https://docs.stripe.com/billing/testing/test-clocks
- Webhook em CI sem conta: o teste de integração constrói o JSON do evento
  (fixture), calcula `v1 = HMAC_SHA256(secret, "{t}.{body}")`, monta o header
  `Stripe-Signature: t=...,v1=...` e chama o handler axum. Isso testa
  verificação de assinatura, tolerância de timestamp, dedup por event id e a
  máquina de estados do plano, tudo offline. É o mesmo espírito do
  `TestState` dos testes atuais; o secret vem de env var de teste como os
  outros (`QUARK_TEST_*`). O detalhe: como o handler "busca o estado atual na
  API", o teste precisa apontar a base URL do cliente Stripe para um mock
  local (o cliente deve receber a base URL por configuração, não hardcoded),
  padrão que o repo já usa para outros clientes HTTP.
- Cartões de teste padrão (4242..., cartões de falha específica) cobrem os
  cenários de decline. Fonte: https://docs.stripe.com/billing/testing

## Recomendações para a spec

- Sem custom domain no lançamento: checkout.stripe.com e billing.stripe.com.
  Reavaliar uma única vez depois da migração para quark.sh (10 USD/mês, CNAME
  + TXT, um domínio cobre checkout, payment links e portal).
- Fluxo de upgrade: backend cria Checkout Session `mode=subscription` com
  `customer`, `client_reference_id=tenant_id`, `subscription_data[metadata]`
  com tenant_id, `currency` explícito escolhido pelo backend, `locale` do
  painel, e redireciona para `session.url`. Payment Links descartados.
- Moeda: multi-currency prices com USD default e BRL em `currency_options`
  (ou o inverso); backend força a moeda via parâmetro `currency` da session;
  lembrar que `customer.currency` trava após a primeira subscription, então a
  moeda é decisão de primeiro upgrade, por tenant.
- Portal resolve o self-service: habilitar switch plan com a lista explícita
  Starter/Pro/Business, quantities OFF, proração de tempo padrão, downgrade
  no fim do período opcional, cancelamento com motivo. Backend só cria a
  portal session.
- Webhook: um endpoint, seis eventos (`checkout.session.completed`,
  `customer.subscription.created/updated/deleted`, `invoice.paid`,
  `invoice.payment_failed`). Verificação v1 manual com hmac+sha2 (tolerância
  5 min, comparação constante), dedup por `event.id` persistido, resposta 2xx
  rápida, e o handler sempre faz GET da subscription na API antes de gravar o
  plano (payload é gatilho, não fonte).
- Mapeamento de status para plano efetivo: `active`/`trialing` e `past_due`
  mantêm o pago (past_due com aviso); `unpaid`/`canceled`/
  `incomplete_expired`/`paused` rebaixam para Free. Dashboard: Smart Retries
  on (8 tentativas / 2 semanas), esgotou vira `canceled`.
- Sem SDK: cliente Stripe fino com reqwest (base URL configurável para mock em
  teste, `Stripe-Version` fixado, connect_timeout padrão do repo). Zero crates
  novos; async-stripe fica documentado como plano B se a superfície crescer.
- Fiscal: lançar sem Stripe Tax (não suporta merchant sediado no Brasil);
  NFS-e/ISS é processo fora do Stripe; preços com imposto embutido; monitorar
  thresholds externos quando houver volume.
- Testes: unit/integration offline assinando payload com whsec de teste;
  test clocks no sandbox para validar renovação e dunning de ponta a ponta;
  `stripe listen` no dev local.
