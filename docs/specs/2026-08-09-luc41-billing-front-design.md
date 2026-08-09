# Front de billing: a grade de planos no painel (LUC-41)

Design da tela de billing do painel e do tratamento global de limite de plano.
Fecha a superfície de usuário sobre o backend das fases 1 e 2
(`docs/specs/2026-08-03-planos-e-entitlement-design.md`,
`docs/specs/2026-08-08-luc41-fase2-stripe-design.md`). Usa o design system
sincronizado do painel (`.design-sync/conventions.md`).

## 1. Escopo

Entra: `GET /admin/billing/catalog` no backend (EE), a tela
`/settings/billing` com a grade de comparação dos planos, o fluxo de
upgrade via Checkout e portal, o tratamento global do `402` no client do
painel, e o redirect amigável do login recusado por teto de membros
(LUC-148).

Não entra: página de preços pública (marketing, LUC-40), Fase 3 (soft cap),
qualquer mudança na grade em si.

## 2. Decisões

### D1. A grade vem do backend, preços vêm do Stripe

A tela mostra os 5 planos lado a lado com limites, features e preços. A
decisão D6 da fase 1 proíbe o painel de carregar cópia própria da grade,
então nasce `GET /admin/billing/catalog` (EE): limites e features saem do
catálogo em código (`Plan::ALL`, fase 1); preços saem do Stripe pelas 6
lookup keys, mensal e anual, USD e BRL no mesmo price (multi-currency).
Nenhum preço em código ou env, coerente com a D3 da fase 2.

### D2. Cache longo de preços, com stale em falha

Preço muda raramente e a chamada ao Stripe custa latência. O backend guarda
o resultado das lookup keys num cache de processo (moka, padrão do repo) com
TTL de 12 horas, e em falha do Stripe serve o valor velho que tiver em vez
de quebrar a grade. O painel não re-busca por navegação dentro da sessão.

### D3. Ver é de todos, comprar é do Owner

O catálogo responde para qualquer membro autenticado (escopo de leitura,
`admin_guard`): a grade é informativa. As ações continuam Owner-only no
backend (fase 2); o painel reflete isso desabilitando o botão de upgrade
para não-Owner, com tooltip explicando.

### D4. Sem billing configurado, a grade degrada para informativa

Num self-host Enterprise sem as env `QUARK_STRIPE_*`, o catálogo devolve
`prices: null` e a tela mostra a grade de limites sem botões de compra. Os
endpoints de checkout e portal continuam 404, como na fase 2. Na edição
Community a rota nem existe (o painel Community não monta as rotas EE).

### D5. 402 é interceptado uma vez, no client central

`web/src/lib/api.ts` já tem o precedente do `setUnauthorizedHandler` para o
401. O 402 ganha o mesmo tratamento: `ApiError` passa a carregar o corpo
estruturado (`error`, `limit`, `allowed`, `upgrade_to`), e um
`setPlanLimitHandler` global dispara um toast com o limite nomeado e a ação
"Ver planos", que navega para `/settings/billing?highlight=<upgrade_to>`.
Nenhuma tela precisa tratar limite de plano individualmente. Na Community o
backend nunca emite 402, então o caminho é inerte e o stub não muda.

### D6. Assinante gerencia no portal, não em checkout novo

O backend responde `409 subscription_active` num segundo checkout (fase 2).
A tela usa isso: para quem já tem assinatura ativa, o botão do card vira
"Gerenciar no portal" e abre o Customer Portal, onde upgrade, downgrade e
cancelamento já vivem. O painel não duplica gestão de assinatura.

### D7. Login recusado por teto vira redirect, não JSON cru

O `member_limit_reached` do callback OIDC (LUC-148) hoje aparece como JSON
no browser. Passa a redirecionar para `{panel}/login?error=member_limit_reached`
usando a mesma base de URL que o callback já usa no sucesso, e a tela de
login mostra a mensagem: o workspace está no limite de membros do plano,
fale com o administrador. Os demais erros do callback não mudam nesta fatia.

## 3. Contrato do catálogo

```
GET /admin/billing/catalog        (EE, admin_guard, escopo de leitura)

200 {
  "current_plan": "starter",
  "currency_locked": "brl" | "usd" | null,
  "prices_available": true,
  "plans": [
    {
      "plan": "free",
      "limits": { "domains": 3, "members": 1, "automation_per_month": 100,
                   "tracked_clicks_per_month": 50000, "retention_days": 30 },
      "features": [],
      "prices": null
    },
    {
      "plan": "starter",
      "limits": { ... },
      "features": ["webhooks", "integrations"],
      "prices": {
        "monthly": { "usd_cents": 400, "brl_cents": 1900 },
        "yearly":  { "usd_cents": 4000, "brl_cents": 19000 }
      }
    },
    ... pro, business ...
    { "plan": "custom", "limits": { tudo null }, "features": [...], "prices": null }
  ]
}
```

`currency_locked` vem da moeda do customer Stripe do tenant quando existe
(cacheada junto dos preços); `null` significa moeda ainda livre. Free e
Custom têm `prices: null` sempre (nada a comprar; negociado). Com billing
desligado, `prices_available: false` e todo `prices` é `null`.

## 4. A tela

Rota `settings/billing` em `eeRoutes`, componente lazy
`web/src/ee/Billing.tsx`, padrão das telas EE existentes (Members, Domains).

- PageHeader com título e eyebrow mono, como as demais telas.
- Toggle mensal/anual (anual com o selo "2 meses grátis"); toggle USD/BRL,
  escondido quando `currency_locked`.
- 5 cards: plano atual com destaque de accent e selo "Plano atual"; Custom
  com "sob consulta" e link `mailto:` para o contato comercial, numa
  constante única no componente (`contato@quarkus.com.br` até a migração de
  domínio da LUC-147). `?highlight=<plan>` (vindo do toast de 402) realça o
  card indicado.
- Botão por card pago: Owner vê "Fazer upgrade" chamando
  `POST /admin/billing/checkout { plan, cycle, currency }` e redirecionando
  para a URL; não-Owner vê o botão desabilitado com tooltip; `409` troca o
  estado da tela para "assinatura ativa" e o botão vira "Gerenciar no
  portal" (`POST /admin/billing/portal`).
- `?checkout=success` mostra toast de sucesso (o plano pode levar segundos
  para virar via webhook; a tela re-busca `GET /admin/plan` algumas vezes);
  `?checkout=cancel` mostra toast neutro.
- Sem `prices_available`, os botões de compra somem.
- Strings via I18nProvider, EN e PT como o resto do painel.

## 5. Testes

- Backend (`tests/billing_it.rs`): catálogo com billing configurado (mock
  HTTP nas lookup keys, preços presentes, cache exercitado) e sem billing
  (`prices_available: false`); qualquer membro lê (não só Owner).
- Painel (`web/src/ee/Billing.test.tsx`): grade renderiza do catálogo
  mockado; botão desabilitado para não-Owner; caminho do 409 vira portal;
  `?checkout=success` mostra o toast.
- Painel (`web/src/lib/api.test.ts`): 402 com corpo estruturado dispara o
  `setPlanLimitHandler` com os campos certos.
- Painel: tela de login com `?error=member_limit_reached` mostra a mensagem.
- Backend: callback com quota negada redireciona em vez de responder JSON.

## 6. Fora de escopo e notas

A página `/settings/billing` é o `success_url` que a fase 2 já emite; com a
rota existindo, o retorno do checkout deixa de cair em `/links`. O toast de
402 usa o Toaster global existente. Telas EE seguem fora do design system
publicado no claude.ai/design (decisão em aberto registrada na memória do
design sync); a tela nova usa os componentes e tokens do DS do repo.
