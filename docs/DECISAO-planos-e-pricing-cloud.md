# Decisão: planos e pricing do quark cloud (LUC-64)

Fecha as decisões que ficaram em aberto na pesquisa de mercado de 2026-07-18
(`docs/research/2026-07-18-cloud-pricing-plans.md`), atualizada com o benchmark
do Short.io de 2026-07-24 e com o que mudou no produto desde então. Decidido em
2026-08-03.

Preço não é compromisso público até a página de preços existir. O que este
documento fixa é a **estrutura**: os eixos, o comportamento no estouro, o
vocabulário e o que cada degrau libera.

## 1. O concorrente de referência

O Short.io. Escolhido pelo dono como a régua, e é a comparação mais honesta:
mesmo produto, mesmo público, preço público e detalhado.

Grade deles, capturada do DOM em 2026-07-24:

| Plano | USD/mês | BRL/mês | Câmbio implícito |
|---|---|---|---|
| Grátis | 0 | 0 | — |
| Hobby | 5 | 25 | 5,00 |
| Pessoal | 18 | 80 | 4,44 |
| Equipe | 48 | 200 | 4,17 |
| Empresa | 148 | 650 | 4,39 |

O câmbio implícito varia 20% entre degraus. Isso não é conversão de FX: são
pontos de preço locais fixados à mão. O brasileiro paga o pior câmbio no degrau
mais barato e o melhor no degrau que eles marcam como melhor oferta, o que
empurra o comprador local para cima da escada com mais força que o americano.

## 2. Três mecanismos que valem copiar

Do benchmark (`docs/research/2026-07-24-shortio-benchmark/dossies/A4-pricing.md`):

1. **O produto nunca quebra, só a visibilidade.** Redirecionamento é ilimitado
   nos cinco planos deles, inclusive o grátis. O que é medido são os cliques
   monitorados, e o FAQ confirma que continuam coletando e escondem o que passa
   do teto. O upgrade destrava retroativamente o que o usuário já gerou.
2. **Link manual e automação são quotas diferentes.** O caminho manual é
   ilimitado; API e lote são medidos. O que eles cobram é automação.
3. **SSO tem preço de tabela e é autosserviço.** US$148/mês. Bitly, Rebrandly,
   Bl.ink e Dub escondem SSO atrás de "fale com vendas". É o ativo comercial
   mais forte que eles têm.

## 3. Decisões

### D1. Estouro de plano: soft cap, o redirect nunca para

Passou do teto de cliques monitorados, o quark **continua gravando** e esconde
a analytics acima do teto. O upgrade destrava retroativo.

Redirect não é gated em plano nenhum, em nenhuma circunstância. É a mesma regra
que a LUC-146 já escreveu para licença vencida no self-host: um encurtador que
para de redirecionar por questão comercial é um incidente, não um modelo de
negócio.

Custo aceito conscientemente: guardar dado que o cliente não vê. O TTL de
retenção por tier é o que limita esse custo, e o destravamento retroativo é a
alavanca de conversão que paga por ele.

### D2. Cobrar pelo que custa, liberar o que não custa

O princípio que decide cada linha da grade.

**Custa:** gravação e storage de analytics, retenção, domínio verificado
(operação, não certificado), suporte.

**Não custa:** toda a lógica do caminho de redirect. Regras geo/device, A/B,
deep link, senha, TTL, max-visits, QR, construtor de UTM. É CPU desprezível e
zero storage marginal.

Consequência, e é o diferencial mais afiado contra o Short.io: eles põem **porta
binária** em deep link, senha e segmentação por país. O quark libera os três no
grátis sem perder um centavo.

### D3. Não cobrar por assento

Assento é porta de tier, nunca unidade de cobrança. O Short.io, a referência,
também não cobra por assento: vai de 1 membro nos planos baixos a ilimitado no
Equipe.

Ganho de engenharia junto: sem sincronizar quantidade com o Stripe, sem
proração, sem a conta do cliente variando sozinha quando alguém entra no time.

### D4. Moeda: USD como base, BRL fixado à mão

Preço multi-moeda no mesmo produto do Stripe, com pontos locais redondos em vez
de conversão. É o que o Short.io faz, e evita amarrar o preço local ao dólar do
dia.

### D5. Edição não é plano

`Community` e `Enterprise` são **edições**: código e licença, o que a LUC-19
entregou (`src/ee/`, `LicenseStatus`). Os planos do cloud têm nomes próprios, e
**nenhum deles se chama Enterprise**.

O precedente é o GitLab: EE é a base de código, Premium e Ultimate são os
planos. Sem essa separação, "Enterprise" significaria duas coisas ao mesmo
tempo em código, painel e página de preços, porque os eixos são independentes:
o cloud roda um binário Enterprise com muitos tenants em planos diferentes, e um
self-host pode ser Enterprise sem ser cliente do cloud.

### D6. Posicionamento de preço: abaixo em toda a escada

| | quark USD | quark BRL | Short.io BRL | Desconto |
|---|---|---|---|---|
| Free | 0 | 0 | 0 | — |
| Starter | 4 | 19 | 25 | 24% |
| Pro | 14 | 59 | 80 | 26% |
| Business | 39 | 149 | 200 | 25% |
| Custom | sob consulta | sob consulta | 650 | — |

Anual: dois meses grátis (~17% off), que é o padrão do mercado.

Os pontos em USD e em BRL são independentes, como os deles. O câmbio implícito
cai conforme sobe a escada (4,75 / 4,21 / 3,82), o que barateia relativamente os
degraus altos para o comprador brasileiro. É deliberado e é a mesma direção que
o Short.io tomou.

**O motivo do desconto importa.** Não é que o quark entregue menos: o doc de
julho já registrava que o quark entrega quase toda feature que esses fornecedores
usam como gate, o que é incomum para um produto pré-receita. Cruzando com o
benchmark:

| Feature | Short.io cobra | quark |
|---|---|---|
| SSO | R$650/mês | tem: OIDC por tenant + realm Keycloak |
| Deep links | Equipe, R$200 | tem, `docs/DEEP-LINKING.md` |
| Proteção por senha | porta binária paga | tem, `docs/LINK-PASSWORD.md` |
| Segmentação por país | porta binária paga | tem, regras geo/device |

O que eles têm a mais é **confiança**: histórico de uptime, suporte, SLA
contratual, ecossistema e marca. Isso justifica desconto, mas é um desconto
**temporário e amarrado a track record**, que sobe conforme o histórico se
acumula. Justificar preço baixo com "entrego menos" seria subprecificar para
sempre.

## 4. A grade

| | Free | Starter | Pro | Business | Custom |
|---|---|---|---|---|---|
| Redirect | ilimitado | ilimitado | ilimitado | ilimitado | ilimitado |
| Links manuais | ilimitado | ilimitado | ilimitado | ilimitado | ilimitado |
| Automação (API/lote) | 100/mês | 5k/mês | 50k/mês | 500k/mês | custom |
| Cliques monitorados | 50k/mês | 250k/mês | 1M/mês | 5M/mês | custom |
| Retenção de analytics | 30 dias | 1 ano | 2 anos | 3 anos | custom |
| Domínios | 3 | 10 | 50 | ilimitado | ilimitado |
| Membros | 1 | 3 | 10 | ilimitado | ilimitado |
| Geo/device, A/B, deep link, senha, TTL, QR, UTM | sim | sim | sim | sim | sim |
| Webhooks e canais | não | sim | sim | sim | sim |
| Sheets, GA4/Meta CAPI, pixels | não | sim | sim | sim | sim |
| Health monitoring, scopes de token | não | não | sim | sim | sim |
| SSO/OIDC por tenant + Keycloak | não | não | não | sim | sim |
| SLA, audit log, infra dedicada | não | não | não | não | sim |

Quatro escolhas que divergem do doc de julho:

- **Link manual não é medido.** Uma linha no banco não custa. O que custa é
  volume por API, então é a automação que é medida.
- **Free com 50k cliques, não 10k.** O Free do Short.io dá 50k. Ser mais
  mesquinho que a referência destruiria a cunha.
- **3 domínios no Free**, contra 1 na proposta de julho. Certificado é Let's
  Encrypt e o custo marginal é próximo de zero.
- **SSO no Business**, não no topo sob consulta. É autosserviço, como no
  Short.io, porque esconder atrás de vendas é o erro que os outros quatro
  cometem.

## 5. As pendências de julho, resolvidas

**#1 Medir clique ou link.** Resolvido em D1 e D2: cliques monitorados com soft
cap, automação medida, link manual livre.

**#2 Retenção tem custo real.** A retenção do Pro caiu de 3 anos (proposta de
julho) para 2, e 3 anos só no Business. Prometer retenção longa antes de conhecer
a curva de custo do ClickHouse é assumir passivo sem número. A curva precisa ser
medida antes de qualquer promessa pública; até lá, estes números são teto de
projeto e não compromisso.

**#3 Isolamento de infra por tenant.** Fica no Custom, como SKU de infra
dedicada. Não vale construir isolamento antes de existir um tenant barulhento
real: é otimização para um problema que ainda não tem forma.

**#4 Custo da API do Claude.** **Não se aplica hoje.** O quark não tem nenhuma
feature de IA, e o doc de julho já marcava isso como roadmap. A decisão fica
registrada para quando existir: nunca no Free, e como add-on medido em créditos,
nunca embutido no plano. COGS variável por request não pode entrar em plano de
preço fixo sem teto.

**#5 Domínios eram roadmap.** Resolvido pelo produto: LUC-82 e LUC-86 estão
entregues, então domínio é eixo de verdade e entra na grade.

**#6 Moeda.** Resolvido em D4.

**#7 Abuso no Free.** É a contrapartida direta de D1 mais 50k cliques no grátis:
o quark passa a pagar storage de tenant que não paga. Vira requisito, não
enfeite: e-mail verificado obrigatório, rate limit menor que o dos pagos, e
scanning de link (que é roadmap, não existe). Sem isso, o Free de um encurtador
é ímã de spam.

**#8 Assentos.** Resolvido em D3.

## 6. O que isto destrava

A LUC-41 (billing) estava bloqueada por não saber o eixo de cobrança. Com D1 a
D6 a espinha fica especificável:

- Cobrança é **por assinatura de plano**, sem quantidade e sem uso reportado ao
  Stripe. Não há medição para faturar, só medição para aplicar teto.
- O que o backend precisa saber por tenant é o **plano** e o **estado da
  assinatura**, não um contador enviado ao gateway.
- O soft cap é decisão de leitura de analytics, não de cobrança, então vive
  longe do caminho de pagamento.

Isso simplifica a LUC-41 bastante: sem preço medido, sem reporte de uso, sem
proração de assento.

## 7. O que continua em aberto

- **Os números são teto de projeto, não compromisso.** Cliques, retenção e
  automação por tier precisam bater com a curva de custo real do ClickHouse
  antes de virar página pública.
- **Nome do quarto degrau.** "Business" é provisório. Precisa sobreviver ao
  teste de não colidir com "edição Enterprise" nem soar como tier de vendas.
- **Trial.** Nada decidido sobre teste grátis dos planos pagos. O mercado
  costuma usar 14 dias sem cartão; fica para a página de preços.

## Fontes

- `docs/research/2026-07-18-cloud-pricing-plans.md`, a pesquisa de mercado que
  este documento fecha
- `docs/research/2026-07-24-shortio-benchmark/dossies/A4-pricing.md`, o dossiê de
  pricing do Short.io, com a captura de DOM em BRL
- [short.io/pt/pricing](https://short.io/pt/pricing/?currency=BRL), a régua
- `docs/specs/2026-08-03-luc19-open-core-design.md`, para a separação entre
  edição e plano
