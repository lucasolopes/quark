[English](BILLING.md) · **Português**

# Billing

O quark Cloud cobra pelo Stripe. Esta página cobre o que um Owner vê e faz; o
lado do operador (criar products, ligar o webhook) está em
[`RUNBOOK-stripe.md`](RUNBOOK-stripe.md). Para a grade de planos em si (o que
cada plano libera, como as cotas são aplicadas), veja [`PLANS.md`](PLANS.md).
Para preços, veja
[`DECISAO-planos-e-pricing-cloud.md`](DECISAO-planos-e-pricing-cloud.md);
esta página não repete números que já vivem lá.

A Community não tem billing nenhum. Não existe código do Stripe no núcleo
AGPL, nenhuma env var à qual ela reaja, nada para configurar. Tudo abaixo é
Enterprise (`--features ee`), e só roda quando o Stripe está configurado.

## Ligando

Três variáveis de ambiente, as três ou nenhuma:

- `QUARK_STRIPE_SECRET_KEY`
- `QUARK_STRIPE_WEBHOOK_SECRET`
- `QUARK_STRIPE_PANEL_URL`

Se qualquer uma das três estiver ausente ou vazia, o billing fica desligado:
os endpoints de checkout, portal e webhook respondem `404`, exatamente como
se as rotas não existissem. Um build Enterprise self-hosted sem Stripe
continua funcionando por completo; os limites de plano (fase 1) são aplicados
independentemente do gateway de pagamento. Não existe um estado intermediário
de billing meio configurado.

## Como um Owner assina

Assinar é um fluxo hospedado pelo Stripe, não um formulário que o quark
renderiza. O Owner (só o Owner; veja abaixo) escolhe plano e ciclo de
cobrança no painel, e o quark pede ao Stripe uma URL de Checkout Session e
redireciona para lá. Preenchimento de cartão, 3-D Secure e qualquer meio de
pagamento local que o Stripe ofereça acontecem na página do Stripe, nunca no
quark.

Algumas decisões ficam travadas nesse primeiro checkout:

- **Moeda.** O Owner escolhe USD ou BRL no primeiro checkout. O Stripe então
  trava essa moeda no customer; toda cobrança seguinte daquele workspace usa
  a mesma. Trocar a moeda de um customer depois é ação manual do operador
  fora do Stripe (veja "Ainda não suportado" abaixo), não algo que o produto
  faz sozinho.
- **Trial.** Um workspace que nunca teve assinatura ganha 14 dias grátis, sem
  cartão, uma única vez. A marca não é o trial já ter sido usado antes; é se
  o workspace já teve algum id de assinatura gravado. Reassinar depois de um
  cancelamento não concede um segundo trial.

Só o papel Owner pode iniciar um checkout ou abrir o Customer Portal. Admins,
Members e Viewers recebem `403` dos dois endpoints; a checagem lê a sessão e
o papel de quem chamou, não um escopo de token de API, porque billing é uma
operação de navegador logado, não algo que um token de automação deveria
poder disparar.

## O que o Customer Portal resolve

Assim que um workspace tem um customer no Stripe, o Owner pode abrir o
Customer Portal hospedado pelo Stripe a partir do painel. Essa única
superfície cobre upgrade, downgrade, cancelamento, troca do cartão cadastrado
e download de faturas anteriores. O quark não constrói equivalente próprio de
nenhuma dessas coisas; o portal é o lugar único para trocar ou sair de um
plano depois que já se é cliente pagante.

## Estados do plano

O quark mantém o status da assinatura no Stripe e a lookup key do preço
sincronizados com o plano do workspace a cada webhook relevante. Nem todo
status do Stripe mantém o plano que a assinatura pagou:

| Status no Stripe | Efeito no plano do workspace |
|---|---|
| `active` | Mantém o plano pago. |
| `trialing` | Mantém o plano pago (é a janela do trial gratuito). |
| `past_due` | Mantém o plano pago, durante a janela de Smart Retries do Stripe. |
| `canceled` | Rebaixa para Free. |
| `unpaid` | Rebaixa para Free. |
| `incomplete_expired` | Rebaixa para Free. |
| `paused` | Rebaixa para Free. |

`past_due` propositalmente não rebaixa na hora: um cartão que falhou uma vez
e é cobrado com sucesso alguns dias depois não deveria ter interrompido o
plano do cliente nesse meio-tempo. O rebaixamento só acontece quando o Stripe
desiste (ou a assinatura é cancelada explicitamente).

## Downgrade nunca apaga nada

Cair para Free, seja por um downgrade explícito no portal ou pela tabela de
dunning acima, nunca apaga um recurso que ficou acima do teto do Free. Um
workspace com 8 domínios que cai no Free (limite: 3) mantém os 8; só não
consegue criar um nono até voltar abaixo do limite ou estar num plano cujo
teto permita. A camada de plano só bloqueia criação nova, o mesmo ponto de
aplicação que a fase 1 já usa para toda outra cota. Não existe job de fundo
que apaga recursos de um workspace até caber no teto do novo plano.

## Ainda não suportado

- **Troca de moeda de um customer existente.** O Stripe não deixa trocar a
  moeda de um customer depois de definida; mudar isso é ação manual do
  operador (tipicamente: cancelar e reassinar com a moeda nova), não um fluxo
  self-service.
- **Tela de billing no painel e o aviso de upgrade no 402.** O painel ainda
  não renderiza plano/uso nem uma chamada para ação quando uma requisição
  volta `402`. Isso chega junto com a landing page de billing.
- **Domínio próprio no checkout.** Checkout e portal usam o domínio do
  próprio Stripe até o suporte a domínio customizado (LUC-147) chegar.
- **Soft cap, os contadores mensais e o teto de automação.** Isso é fase 3.
- **Stripe Tax.** Não está ligado; veja o runbook para como fica a nota
  fiscal de um merchant brasileiro sem ele.
