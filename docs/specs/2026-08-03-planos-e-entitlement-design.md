# Planos e entitlement: aplicar a grade sem cobrar ainda

Design da primeira fatia do billing. Fecha o modelo de plano e a aplicação de
limites; **não** integra gateway de pagamento nenhum.

A grade de planos vem de `docs/DECISAO-planos-e-pricing-cloud.md` (LUC-64). A
separação entre core e Enterprise vem de
`docs/specs/2026-08-03-luc19-open-core-design.md` (LUC-19).

## 1. Por que esta fatia existe separada

A LUC-41 (billing) junta seis superfícies: modelo de plano, entitlement, quota,
soft cap de analytics, Stripe e tela de billing. Isso não cabe num plano de
implementação só, e as partes têm dependências diferentes.

| Fatia | O quê | Depende de |
|---|---|---|
| **1, esta spec** | plano no tenant, catálogo, aplicação dos limites, painel lendo do servidor | nada |
| 2 | Stripe: customer, checkout, portal, webhooks idempotentes | fatia 1 |
| 3 | soft cap: contador mensal de cliques, esconder analytics acima do teto | fatia 1 |

A fatia 1 tem valor sozinha: faz o cloud **aplicar** a grade antes de existir
cobrança, e é testável sem serviço externo nenhum. As fatias 2 e 3 são issues
próprias.

## 2. Decisões

### D1. Plano no banco, catálogo em código

O tenant guarda **qual** plano tem. **Quais** são os limites de cada plano é
`const` em Rust, versionada junto da feature que ela limita.

Evidência, do Dub, que é o análogo open source mais próximo e roda negócio de
verdade: o catálogo de limites é struct em código
(`packages/utils/src/constants/pricing/pricing-plans.tsx`, `PlanDetails.limits`),
e os price IDs do Stripe também estão em código, incluindo uma lista
`LEGACY_PRO_PRICE_IDS` com preços de 2023 e 2024.

Essa última parte é o que decide contra as alternativas. Grandfathering acontece
mapeando **vários price IDs para um plano, em código**. Com o catálogo no Stripe,
cada mudança de preço viraria override por cliente; com catálogo em tabela
editável, viraria migração a cada mudança.

Consequência aceita: mudar limite é deploy, e vale imediatamente para todos
naquele plano. Para um produto pré-receita isso é vantagem, porque não há
cliente em preço antigo para preservar.

**Descartado: Stripe como fonte da verdade (Entitlements API).** Existe e é
real, mas o `src/ee/` roda em self-host **sem Stripe nenhum**. Se o entitlement
viesse do Stripe, o build Enterprise self-hosted não conseguiria responder "esse
tenant pode X". A camada de plano precisa ser independente do gateway por
construção. Some-se que a própria documentação do Stripe recomenda persistir os
entitlements localmente por performance, ou seja, sobra estado local de todo
jeito.

**Descartado: catálogo em tabela editável.** Compra flexibilidade que ninguém
pediu (mudar limite sem deploy) e paga com migração, tela de admin e o risco de
tenant com limite órfão quando a feature muda.

### D2. Isto é Enterprise, e a Community nunca limita

Plano só existe operando o quark como serviço para terceiros, então cai do lado
EE pela regra da LUC-19.

Corolário obrigatório: **a edição Community não aplica limite de plano nenhum.**
Um self-host AGPL é livre e ilimitado. Limitar ele contradiria a decisão inteira
do open core.

### D3. Seam no core, implementação na EE

Os pontos onde o gate é aplicado estão dos dois lados: webhook é core
(`src/api/webhooks_api.rs`), domínio e convite são EE. Então o core não pode
depender de `src/ee/`, e a solução é a mesma da LUC-145:

```
src/api/entitlement.rs        seam. Community sempre permite.
src/ee/plan.rs                catalogo: Plan, Feature, Limits.
src/ee/api/entitlement.rs     implementacao: le o plano do tenant, consulta o catalogo.
```

Duas funções selecionadas por `cfg`, não trait: a escolha é de compilação e
nunca varia em runtime.

### D4. O compilador exige a decisão

O modo de falha que importa não é limite errado, é **feature nova que ninguém
lembrou de classificar**. Uma lista (`features: &[Feature]`) falha em silêncio:
esquecer de incluir deixa a feature negada em todo plano, sem erro.

Por isso o catálogo é `match` exaustivo, sem braço curinga:

```rust
impl Plan {
    pub fn allows(self, f: Feature) -> bool {
        // Sem `_ =>`, de proposito. Adicionar variante em `Feature` quebra a
        // compilacao aqui e lista cada plano que precisa decidir.
        match (self, f) {
            (Plan::Free, Feature::Webhooks) => false,
            (Plan::Starter, Feature::Webhooks) => true,
            // ...
        }
    }
}
```

Mesma disciplina no numérico: `Limits` é struct de campos nomeados e **não
implementa `Default`**. Adicionar um campo obriga cada `const` de plano a
preencher, porque não existe `..Default::default()` para escapar.

Um teste fecha o resto: itera `Plan::ALL` × `Feature::ALL` e afirma que a
liberação é monotônica na escada, ou seja, nenhum plano superior nega o que um
inferior libera.

### D5. A checagem não mora no `admin_guard`

A nota técnica da LUC-41 pede o gating "no mesmo ponto que já resolve
tenant/Principal no `admin_guard`, não duplicado por rota". A intenção é não
duplicar, e ela é respeitada, mas o lugar muda.

`admin_guard` (`src/api/guard.rs:31`) resolve credencial e devolve `Principal`
**sem carregar a linha do tenant**. Ele roda em toda requisição admin, inclusive
leitura sem nenhuma implicação de plano. Colocar consulta de plano ali taxaria
tudo.

O não-duplicar se cumpre com **uma função só**, chamada apenas onde há gate.
Nesta fatia são os gates que não precisam de contador mensal: criar webhook,
conectar Sheets ou pixel, ligar SSO por tenant (binários), e criar domínio ou
convidar membro acima do teto (contagem de linhas, que já dá para fazer com uma
consulta). O teto de automação por API entra na fatia 3, junto da máquina de
contagem mensal que ele compartilha com o soft cap.

Nenhuma checagem no caminho de redirect, nunca.

### D6. O painel não conhece a grade

Terceira cópia que dessincroniza sozinha. Se a tela tiver a grade hardcoded, ela
diverge do backend na primeira mudança.

`GET /admin/plan` devolve o plano do tenant, seus limites, o uso corrente e as
features liberadas. A tela renderiza a partir disso.

Fica de fora da garantia a página de preços de marketing, que é conteúdo e vai
divergir por natureza. Ela é cópia, não fonte, e isso precisa estar escrito nela.

### D7. Estouro responde 402, não 403

`403 Forbidden` significa "você não tem permissão". Aqui a pessoa tem permissão;
falta plano. `402 Payment Required` diz isso.

O corpo nomeia o limite atingido, o valor e o plano que resolve, para o painel
conseguir montar a chamada de upgrade sem adivinhar. O Dub usa 403 com mensagem;
a diferença é deliberada.

### D8. Gate novo vale para feature nova

Feature que já saiu livre continua livre. Fechar depois é tirar o que já foi
dado: rende pouco e queima confiança. É o mesmo raciocínio que manteve o login
OIDC de instância única no core (D2.1 da spec da LUC-19).

Isso choca de frente com o que a Fase 1 realmente faz: webhooks, Sheets,
pixels, SSO, domínios e membros já existiam antes desta fase e, a partir
dela, passam a ser limitados por plano. Pela letra de D8 isso seria "tirar o
que já foi dado". A regra continua valendo porque a premissa dela (existe
gente pagando, e fechar tira algo que ela já usava) ainda não se aplica: o
quark Cloud não foi lançado, não existe cliente pagante, e ninguém hoje tem
uma dessas features "dada" para ser tirada. Fechar agora, antes do
lançamento, é definir a grade de lançamento, não fazer um take-back. D8 volta
a valer no sentido literal a partir do lançamento: qualquer feature que saia
livre depois desse ponto, para um tenant que já paga, segue a regra sem
exceção.

Regra para classificar feature nova, na ordem:

| Pergunta | Se sim |
|---|---|
| É lógica do caminho de redirect? | livre em todos, inclusive Free |
| Serve para operar para terceiros (SSO, audit, infra dedicada)? | degrau alto |
| Custa por uso (storage, egress, API de terceiro)? | medido, com teto por plano |
| Implica suporte ou operação nossa? | degrau pago |
| Nenhuma das anteriores | livre |

## 3. Modelo de dados

Migração no `init_schema`, seguindo o padrão já usado pela LUC-86
(`ALTER TABLE tenants ADD COLUMN IF NOT EXISTS primary_domain_id BIGINT`):

```sql
ALTER TABLE tenants ADD COLUMN IF NOT EXISTS plan TEXT NOT NULL DEFAULT 'free';
ALTER TABLE tenants ADD COLUMN IF NOT EXISTS plan_limits JSONB;
```

`plan_limits` é `NULL` em todo plano de catálogo. Só o tier `Custom` preenche,
que é onde existe negociação. Não copiamos todos os limites em toda linha como o
Dub faz: ele precisa disso por ter cliente em preço antigo, e o quark não tem.

O LMDB (backend OSS) não ganha nada disso: em Community não há plano.

`Plan` e `Limits` são tipos de `src/ee/`, então `Tenant` (core, nomeado pelo
trait `Store`) **não** cresce um campo tipado. O acesso é por método próprio no
store, como `get_primary_domain_id`/`set_primary_domain` já fazem.

## 4. Fluxo

```
handler (core ou EE)
  -> entitlement::require(&st, tenant, Feature::Webhooks)      binaria
  -> entitlement::require_quota(&st, tenant, Quota::Domains, atual)  numerica
       Community: Ok sempre
       EE: cache tenant->Plan (moka, TTL + invalidacao na troca)
           -> Plan::allows / Limits
           -> Ok | Err(EntitlementError { limite, valor, plano_que_resolve })
```

O cache segue o padrão que o repo já usa em `TenantOidcCache` e no
`HostRouter`: moka com TTL, invalidado explicitamente quando o plano muda.

## 5. Testes

- Unitário do catálogo: monotonicidade na escada (`Plan::ALL` × `Feature::ALL`).
- Unitário: `Limits` de cada plano bate com a tabela de `DECISAO-planos-e-pricing-cloud.md`.
- Integração EE: tenant Free recebe 402 ao criar webhook; tenant Starter cria.
- Integração EE: teto de domínio e de membro, no limite e um acima.
- Integração Community (`cargo test` sem a feature): os mesmos caminhos passam
  sem limite. É o teste que prova o D2.
- Regressão: o caminho de redirect não chama entitlement. Garantido por não
  existir a chamada, e verificado por busca no gate.

## 6. Fora de escopo

Stripe inteiro (fatia 2). Soft cap de analytics, contador mensal de cliques e
teto de automação por API (fatia 3, os três compartilham a mesma máquina de
contagem mensal). Tela de billing com upgrade, que depende da fatia 2. Trial.

## 7. Em aberto

- **Nome do quarto degrau.** "Business" é provisório, herdado da LUC-64.
- **Números são teto de projeto.** A grade não vira compromisso público antes de
  a curva de custo do ClickHouse ser medida.
- **Quem seta o plano antes da fatia 2.** Sem Stripe, a troca de plano é
  operação manual. A spec assume um caminho administrativo mínimo, restrito ao
  operador do cloud, e não uma tela de self-service.
