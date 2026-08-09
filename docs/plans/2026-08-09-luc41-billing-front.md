# Front de billing (LUC-41) — plano de implementação

> **Para quem executa com agente:** SUB-SKILL OBRIGATÓRIA: use
> `superpowers:subagent-driven-development` (recomendado) ou
> `superpowers:executing-plans` para implementar tarefa a tarefa. Os passos usam
> checkbox (`- [ ]`) para acompanhamento.

**Objetivo:** a tela `/settings/billing` com a grade de comparação dos planos,
upgrade via Checkout, portal, tratamento global do 402 e o redirect amigável do
login recusado por teto de membros.

**Arquitetura:** um endpoint novo `GET /admin/billing/catalog` (EE) serve a
grade inteira: limites e features do catálogo em código, preços do Stripe pelas
lookup keys com cache de 12h e stale em falha. O painel ganha a tela EE
`Billing.tsx` (rota em `eeRoutes`), um interceptor de 402 no client central
(`api.ts`, padrão do handler de 401) e a leitura de `?error=` na tela de login.

**Stack:** Rust/axum + async-stripe 1.0.0-rc.8 (backend); React 19 + react
-router + TanStack Query + sonner + design system do repo (painel).

Spec: `docs/specs/2026-08-09-luc41-billing-front-design.md`.

## Restrições globais

- Trabalho no worktree `.claude/worktrees/luc41-billing-front` (branch
  `worktree-luc41-billing-front`). O checkout principal tem um refactor alheio
  em andamento; nunca tocar fora do worktree.
- Rust: `cargo fmt` e `cargo clippy --all-targets -- -D warnings` limpos nos
  dois modos (sem feature e `--features ee`); `cargo` em `~/.cargo/bin/cargo.exe`;
  Postgres de teste em `QUARK_TEST_DATABASE_URL='postgres://quark_test:quark_test@127.0.0.1:5432/quark_test'`.
- Painel: `npm test`, `npm run test:ee`, `npm run lint` e `npm run typecheck`
  limpos (rodar dentro de `web/`).
- Código e comentários em inglês (regra do usuário); strings de UI via i18n
  EN + PT com a MESMA shape nos dois arquivos (o tipo `MessageKey` deriva de
  `en.ts` e o TS quebra se `pt-BR.ts` divergir).
- Cores/tipografia só por tokens do design system (`.design-sync/conventions.md`);
  nunca hex hardcoded.
- Nada de tipo de `src/ee/` nomeado fora de `src/ee/`; no painel, tela EE vive
  em `web/src/ee/` e entra só pelo barrel `web/src/ee/index.tsx` (o stub
  `web/src/lib/ee-stub.tsx` não muda: rota nova entra por `eeRoutes`, que já
  existe no contrato).
- Assinaturas do async-stripe rc.8 não confirmadas no doc de pesquisa
  (`docs/research/2026-08-08-async-stripe-rc-api.md`) devem ser conferidas em
  docs.rs/<crate>/1.0.0-rc.8 antes de usar; nunca afrouxar comportamento pra
  API caber.

## Estrutura de arquivos

| Arquivo | Responsabilidade |
|---|---|
| `src/ee/stripe/mod.rs` (modificar) | cache de preços do catálogo com stale-on-error |
| `src/ee/api/billing.rs` (modificar) | handler `admin_billing_catalog` |
| `src/ee/api/mod.rs` (modificar) | rota `GET /admin/billing/catalog` |
| `src/api/oidc_login.rs` (modificar) | `MemberLoginDenied::Quota` vira redirect pro painel |
| `tests/billing_it.rs` (modificar) | testes do catálogo e do redirect |
| `web/src/lib/types.ts` (modificar) | tipos do catálogo e do corpo de 402 |
| `web/src/lib/api.ts` (modificar) | chamadas novas + interceptor de 402 |
| `web/src/lib/queries.ts` (modificar) | hooks de query/mutation do billing |
| `web/src/app/App.tsx` (modificar) | registra o handler global de 402 (toast com CTA) |
| `web/src/ee/Billing.tsx` (criar) | a tela da grade |
| `web/src/ee/Billing.test.tsx` (criar) | testes da tela |
| `web/src/ee/index.tsx` (modificar) | rota `settings/billing` em `eeRoutes` |
| `web/src/routes/Login.tsx` (modificar) | mensagem de `?error=member_limit_reached` |
| `web/src/i18n/en.ts`, `web/src/i18n/pt-BR.ts` (modificar) | namespace `billing:` + chave no `login:` |
| `docs/BILLING.md` + `.PT_BR.md` (modificar) | seção da tela do painel |

---

### Task 1: catálogo no backend, com cache de preços

**Files:**
- Modify: `src/ee/stripe/mod.rs` (cache), `src/ee/api/billing.rs` (handler),
  `src/ee/api/mod.rs` (rota)
- Test: `tests/billing_it.rs`

**Interfaces:**
- Consumes: `Plan::ALL`, `Plan::limits()`, `Plan::allows()`, `Feature::ALL`
  (fase 1); `map::lookup_key(plan, cycle)`; `admin_guard(&st, &headers,
  Scope::LinksRead)`; `plan_of(&st, tenant)` (`crate::ee::api::entitlement`);
  `st.store.get_stripe_customer_id`.
- Produces: `GET /admin/billing/catalog` com o contrato da spec §3, e
  `StripeBilling::catalog_prices(&self) -> Option<CatalogPrices>` (cache).

- [ ] **Step 1: teste que falha** — em `tests/billing_it.rs` (reuse
  `state_with_billing`, `seed_session` e o mock HTTP local do teste
  `webhook_subscription_event_flips_the_plan_end_to_end`):

```rust
#[tokio::test]
#[serial_test::file_serial]
async fn catalog_serves_the_grid_with_prices_from_the_mock() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    // Mock answering GET /v1/prices with one price per requested lookup key
    // (extend the existing mock server helper with a /v1/prices route that
    // echoes the lookup_keys[] param into fixture prices: usd unit_amount 400,
    // currency_options.brl 1900, recurring.interval month/year per key).
    let (st, t) = state_with_billing(&spawn_stripe_mock_with_prices().await).await;
    // Any member can read the grid: seed a Viewer session, not an Owner.
    let cookie = seed_session(&st, t, 31, Role::Viewer).await;
    let app = quark::api::router(st.clone());
    let res = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/admin/billing/catalog")
                .header("cookie", &cookie)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["prices_available"], true);
    assert_eq!(v["plans"].as_array().unwrap().len(), 5);
    let starter = &v["plans"][1];
    assert_eq!(starter["plan"], "starter");
    assert_eq!(starter["limits"]["members"], 3);
    assert_eq!(starter["prices"]["monthly"]["usd_cents"], 400);
    assert_eq!(starter["prices"]["monthly"]["brl_cents"], 1900);
    assert_eq!(v["plans"][0]["prices"], serde_json::Value::Null); // free
    assert_eq!(v["plans"][4]["prices"], serde_json::Value::Null); // custom
}

#[tokio::test]
#[serial_test::file_serial]
async fn catalog_without_billing_is_informative_only() {
    if std::env::var("QUARK_TEST_DATABASE_URL").is_err() {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    }
    // State WITHOUT billing (build TestState directly, billing None), same as
    // checkout_is_404_when_billing_is_not_configured does.
    let (st, t) = state_without_billing().await;
    let cookie = seed_session(&st, t, 32, Role::Member).await;
    let app = quark::api::router(st.clone());
    let res = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/admin/billing/catalog")
                .header("cookie", &cookie)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["prices_available"], false);
    assert_eq!(v["plans"][1]["prices"], serde_json::Value::Null);
}
```

  Nota: o guard do handler é `admin_guard` (sessão OU token com
  `Scope::LinksRead`); os testes acima autenticam por cookie de sessão como os
  demais. Extraia `state_without_billing()` do corpo do teste de 404 existente
  se ainda não houver helper.

- [ ] **Step 2: rodar e ver falhar** —
  `QUARK_TEST_DATABASE_URL=... ~/.cargo/bin/cargo.exe test --features ee --test billing_it catalog`
  → FALHA (404, rota não existe).

- [ ] **Step 3: cache de preços em `src/ee/stripe/mod.rs`** — struct e método
  novos (comentários em inglês):

```rust
/// One plan's Stripe prices for the catalog, cents per currency and cycle.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CatalogPrice {
    pub usd_cents: i64,
    pub brl_cents: i64,
}

#[derive(Debug, Clone, Default)]
pub struct CatalogPrices {
    /// lookup_key -> price. Missing key means the dashboard lacks that price.
    pub by_lookup_key: std::collections::HashMap<String, CatalogPrice>,
    /// The tenant-independent part only; the customer's locked currency is
    /// resolved per request, not cached here.
    pub fetched_at: std::time::Instant,
}
```

  Campo novo em `StripeBilling`:
  `pub catalog_cache: tokio::sync::RwLock<Option<CatalogPrices>>` (inicializado
  `RwLock::new(None)` em `from_parts`). Método:

```rust
/// The 6 self-service prices, cached for 12h. On a Stripe failure past the
/// TTL the stale value is served instead of breaking the grid (spec D2);
/// `None` only when there has never been a successful fetch.
pub async fn catalog_prices(&self) -> Option<CatalogPrices>
```

  Implementação: se o cache tem valor com `fetched_at.elapsed() < 12h`, retorna
  clone. Senão busca: UMA chamada `ListPrice::new().lookup_keys(vec![as 6 keys])
  .active(true)` com expand de `currency_options` (confirme em
  docs.rs/async-stripe-product/1.0.0-rc.8 o setter de expand em `ListPrice` e o
  prefixo `data.currency_options`; se `ListPrice` não expuser expand, faça 6
  `RetrievePrice` com expand, também conferindo a assinatura). Monta o mapa
  lookup_key → `CatalogPrice { usd_cents: unit_amount, brl_cents:
  currency_options["brl"].unit_amount }`. Sucesso: grava no cache e retorna.
  Erro: `tracing::warn!` e retorna o valor velho se houver (stale), senão
  `None`.

- [ ] **Step 4: handler em `src/ee/api/billing.rs`**:

```rust
/// `GET /admin/billing/catalog`: the whole plan grid for the panel. Read
/// scope on purpose: any member may LOOK at the grid (spec D3); the actions
/// stay Owner-only. Limits and features come from the code catalog (phase 1
/// D6: the panel never carries its own copy); prices come from Stripe
/// through the 12h cache (spec D2).
pub(crate) async fn admin_billing_catalog(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if !st.multi_tenant {
        return StatusCode::NOT_FOUND.into_response();
    }
    let p = match admin_guard(&st, &headers, Scope::LinksRead).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    let current = crate::ee::api::entitlement::plan_of(&st, p.tenant).await;
    let (prices, currency_locked) = match &st.ee.billing {
        Some(b) => (
            b.catalog_prices().await,
            locked_currency(&st, b, p.tenant).await,
        ),
        None => (None, None),
    };
    let plans: Vec<serde_json::Value> = Plan::ALL
        .into_iter()
        .map(|plan| {
            let l = plan.limits();
            let features: Vec<&'static str> = crate::api::entitlement::Feature::ALL
                .into_iter()
                .filter(|f| plan.allows(*f))
                .map(crate::api::entitlement::Feature::as_str)
                .collect();
            let plan_prices = prices.as_ref().and_then(|cp| {
                let m = crate::ee::stripe::map::lookup_key(plan, Cycle::Monthly)?;
                let y = crate::ee::stripe::map::lookup_key(plan, Cycle::Yearly)?;
                Some(serde_json::json!({
                    "monthly": cp.by_lookup_key.get(m),
                    "yearly": cp.by_lookup_key.get(y),
                }))
            });
            serde_json::json!({
                "plan": plan.as_str(),
                "limits": {
                    "domains": l.domains,
                    "members": l.members,
                    "automation_per_month": l.automation_per_month,
                    "tracked_clicks_per_month": l.tracked_clicks_per_month,
                    "retention_days": l.retention_days,
                },
                "features": features,
                "prices": plan_prices,
            })
        })
        .collect();
    Json(serde_json::json!({
        "current_plan": current.as_str(),
        "currency_locked": currency_locked,
        "prices_available": prices.is_some(),
        "plans": plans,
    }))
    .into_response()
}
```

  `locked_currency`: helper que lê `get_stripe_customer_id(tenant)`; com
  customer, `RetrieveCustomer` (confirme a assinatura em
  docs.rs/async-stripe-core/1.0.0-rc.8, feature `customer` já ligada) e devolve
  `customer.currency` como `Option<String>`; sem customer ou em erro, `None`
  (a moeda livre é o fallback seguro: o Stripe rejeita a errada no checkout).
  Cacheie por tenant junto no `catalog_cache`? NÃO: guarde num
  `moka::future::Cache<TenantId, Option<String>>` com TTL 12h no `EeState`?
  Também não. Decisão do plano: cache simples dentro de `StripeBilling`,
  `RwLock<HashMap<TenantId, (Instant, Option<String>)>>`, mesma janela de 12h,
  mesmo padrão stale do de preços; é pouco tráfego e mantém tudo num lugar.

  Rota em `src/ee/api/mod.rs`, junto das outras de billing:
  `.route("/admin/billing/catalog", get(admin_billing_catalog))`.

- [ ] **Step 5: rodar e ver passar** os dois testes; depois
  `~/.cargo/bin/cargo.exe test --features ee --test billing_it` inteiro.

- [ ] **Step 6: gates e commit** — fmt + clippy dois modos;
  `git add src/ee tests/billing_it.rs && git commit -m "feat(ee): catalogo de planos com precos do Stripe em cache (LUC-41)"`
  (+ linha em branco + `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`,
  igual nos commits seguintes).

---

### Task 2: login recusado vira redirect pro painel

**Files:**
- Modify: `src/api/oidc_login.rs` (o `IntoResponse` de `MemberLoginDenied`)
- Test: `tests/plan_it.rs` (ajustar o teste existente do gate)

**Interfaces:**
- Consumes: `MemberLoginDenied` (LUC-148), `st.ee.billing.panel_url` (EE, via
  cfg) — atenção: `oidc_login.rs` é CORE; ele não pode nomear `StripeBilling`.
- Produces: comportamento novo: `Quota` redireciona para
  `{panel}/login?error=member_limit_reached` quando há uma URL de painel
  conhecida; sem ela, mantém o 402 JSON atual.

- [ ] **Step 1: descobrir a fonte da URL do painel no core.** O core não pode
  ler `st.ee.billing`. Fontes possíveis, nesta ordem: (a) se `AppState` já tem
  um campo core com a URL do painel (procure por `panel` em `src/api/mod.rs` e
  `src/main.rs`), use-o; (b) senão, promova a leitura da env
  `QUARK_STRIPE_PANEL_URL` para um campo core opcional
  `AppState.panel_url: Option<String>` preenchido no boot do `main.rs`
  (a env já existe e é a URL do painel; o nome fica como está para não criar
  env nova), com o `EeState`/`StripeBilling` continuando a ler a mesma env.
  Documente no campo que o core usa isso só para redirects de login.
- [ ] **Step 2: teste primeiro** — em `tests/plan_it.rs`, o teste que hoje
  afirma o 402 do gate passa a afirmar: com `panel_url` configurada no estado
  de teste, a resposta é `303 See Other` (ou `302`, siga o que
  `axum::response::Redirect::to` emite) com `location` terminando em
  `/login?error=member_limit_reached`; sem `panel_url`, continua o 402 JSON.
  Adapte o builder de `tests/common` se precisar de setter para o campo novo.
- [ ] **Step 3: implementar** — no `IntoResponse`/fluxo do callback: o handler
  passa a decidir com o `panel_url` em mãos (mude `member_quota_allows_login`
  ou o ponto de uso para receber `&AppState`, que ele já recebe) e responde
  `Redirect::to(&format!("{panel}/login?error=member_limit_reached"))` no caso
  `Quota` com painel conhecido. `StoreUnavailable` continua 503.
- [ ] **Step 4: rodar plan_it + api::tests; gates; commit**
  `fix(oidc): login recusado por teto redireciona pro painel (LUC-41)`.

---

### Task 3: client do painel — tipos, chamadas e interceptor de 402

**Files:**
- Modify: `web/src/lib/types.ts`, `web/src/lib/api.ts`, `web/src/app/App.tsx`
- Test: `web/src/lib/api.test.ts`

**Interfaces:**
- Produces (tipos):

```ts
export interface CatalogPrice { usd_cents: number; brl_cents: number; }
export interface CatalogPlanPrices { monthly: CatalogPrice | null; yearly: CatalogPrice | null; }
export interface CatalogPlan {
  plan: string;
  limits: { domains: number | null; members: number | null; automation_per_month: number | null;
            tracked_clicks_per_month: number | null; retention_days: number | null };
  features: string[];
  prices: CatalogPlanPrices | null;
}
export interface BillingCatalog {
  current_plan: string;
  currency_locked: string | null;
  prices_available: boolean;
  plans: CatalogPlan[];
}
export interface PlanLimitBody { error: string; limit: string; allowed: number | null; upgrade_to: string; }
```

- Produces (api.ts): `getBillingCatalog(): Promise<BillingCatalog>`
  (`GET /admin/billing/catalog`), `startCheckout(plan, cycle, currency):
  Promise<{ url: string }>` (`POST /admin/billing/checkout`),
  `openPortal(): Promise<{ url: string }>` (`POST /admin/billing/portal`),
  `setPlanLimitHandler(fn: (b: PlanLimitBody) => void)`, e `ApiError` ganha
  campo opcional `planLimit?: PlanLimitBody`.

- [ ] **Step 1: teste que falha** em `web/src/lib/api.test.ts` (siga o padrão
  de spy de `fetch` do arquivo):

```ts
it("parses a 402 body, fires the plan-limit handler and enriches ApiError", async () => {
  const seen: PlanLimitBody[] = [];
  setPlanLimitHandler((b) => seen.push(b));
  vi.spyOn(globalThis, "fetch").mockResolvedValue(
    new Response(JSON.stringify({ error: "plan_limit_reached", limit: "webhooks", allowed: null, upgrade_to: "starter" }), { status: 402 }),
  );
  await expect(api.createWebhook({ url: "https://x.example", events: ["link.created"] })).rejects.toMatchObject({ status: 402, planLimit: { limit: "webhooks", upgrade_to: "starter" } });
  expect(seen).toHaveLength(1);
  expect(seen[0].upgrade_to).toBe("starter");
});
```

- [ ] **Step 2: rodar e ver falhar** — `cd web && npx vitest run src/lib/api.test.ts`.
- [ ] **Step 3: implementar** — em `api.ts`: `let onPlanLimit: (b: PlanLimitBody) => void = () => {};`
  + `setPlanLimitHandler`. Em `jsonOrThrow` (e no padrão manual dos endpoints
  sem corpo), quando `res.status === 402`: leia o texto, tente `JSON.parse`;
  se tiver `error` começando com o padrão (`plan_limit_reached` ou
  `member_limit_reached`), chame `onPlanLimit(body)` e lance `ApiError` com
  `planLimit` preenchido. Extraia um helper `throwApiError(res): Promise<never>`
  usado por `jsonOrThrow` e pelos endpoints void, pra não duplicar. As três
  chamadas novas seguem o padrão de `listInvites`/`createInvite`.
- [ ] **Step 4: registrar o handler global** em `web/src/app/App.tsx` (onde o
  Toaster já mora): num `useEffect` de montagem,

```tsx
setPlanLimitHandler((b) => {
  toast.error(t("billing.limitToast", { limit: b.limit }), {
    action: { label: t("billing.limitToastCta"), onClick: () => router.navigate(`/settings/billing?highlight=${b.upgrade_to}`) },
  });
});
```

  (o `router` importado de `@/app/router`; sonner aceita `action: { label,
  onClick }`. Se App.tsx não tiver acesso ao `t` fora da árvore do
  I18nProvider, resolva com `getMessage`/idioma atual do módulo i18n, seguindo
  como o próprio provider obtém o idioma. ESTA task cria o namespace
  `billing:` nos DOIS arquivos i18n com as duas chaves que usa —
  `limitToast: "Your plan's {limit} limit was reached."` e
  `limitToastCta: "View plans"`, com PT natural no twin — senão o typecheck
  deste gate quebra; as Tasks 4 e 5 estendem o namespace.)
- [ ] **Step 5: testes + lint + typecheck; commit**
  `feat(web): interceptor global de limite de plano no client (LUC-41)`.

---

### Task 4: a tela Billing

**Files:**
- Create: `web/src/ee/Billing.tsx`, `web/src/ee/Billing.test.tsx`
- Modify: `web/src/ee/index.tsx` (lazy + rota `settings/billing`),
  `web/src/lib/queries.ts` (hooks)
- Test: `web/src/ee/Billing.test.tsx` (roda com `npm run test:ee`)

**Interfaces:**
- Consumes: `getBillingCatalog`/`startCheckout`/`openPortal` (Task 3),
  `useMe` (role do workspace atual: `me.memberships.find(m => m.tenant_id ===
  me.current_tenant)?.role === "owner"`), componentes do DS (PageHeader, Card,
  Button, Skeleton, Badge/selo com tokens), `useT()`.
- Produces: rota `settings/billing`; hooks `useBillingCatalog()`,
  `useStartCheckout()`, `useOpenPortal()` em `queries.ts`.

- [ ] **Step 1: testes que falham** (`Billing.test.tsx`, padrão do
  `Members.test.tsx`: spy de fetch + `withProviders`; o componente precisa de
  router por causa de `useSearchParams` → `withRouter: true` ou o default do
  helper):

```tsx
const CATALOG = {
  current_plan: "free",
  currency_locked: null,
  prices_available: true,
  plans: [
    { plan: "free", limits: { domains: 3, members: 1, automation_per_month: 100, tracked_clicks_per_month: 50000, retention_days: 30 }, features: [], prices: null },
    { plan: "starter", limits: { domains: 10, members: 3, automation_per_month: 5000, tracked_clicks_per_month: 250000, retention_days: 365 }, features: ["webhooks", "integrations"], prices: { monthly: { usd_cents: 400, brl_cents: 1900 }, yearly: { usd_cents: 4000, brl_cents: 19000 } } },
    { plan: "pro", limits: { domains: 50, members: 10, automation_per_month: 50000, tracked_clicks_per_month: 1000000, retention_days: 730 }, features: ["webhooks", "integrations"], prices: { monthly: { usd_cents: 1400, brl_cents: 5900 }, yearly: { usd_cents: 14000, brl_cents: 59000 } } },
    { plan: "business", limits: { domains: null, members: null, automation_per_month: 500000, tracked_clicks_per_month: 5000000, retention_days: 1095 }, features: ["webhooks", "integrations", "sso"], prices: { monthly: { usd_cents: 3900, brl_cents: 14900 }, yearly: { usd_cents: 39000, brl_cents: 149000 } } },
    { plan: "custom", limits: { domains: null, members: null, automation_per_month: null, tracked_clicks_per_month: null, retention_days: null }, features: ["webhooks", "integrations", "sso"], prices: null },
  ],
};
const ME_OWNER = { authenticated: true, multi_tenant: true, current_tenant: 1,
  memberships: [{ tenant_id: 1, name: "Acme", slug: "acme", role: "owner" }] };

// mockFetch: route by URL — /admin/billing/catalog -> CATALOG, /admin/me -> me,
// POST /admin/billing/checkout -> { url } | 409, POST /admin/billing/portal -> { url }.

it("renders the five plan cards and marks the current one", async () => { /* owner me; expect card titles Free..Custom, badge "Current plan" on Free, prices "R$ 19" after switching currency or "$4" default */ });
it("disables the upgrade button for a non-owner with a tooltip", async () => { /* role member; button disabled */ });
it("switches to the portal path on 409", async () => { /* checkout mock answers 409 {error:"subscription_active"}; after click, portal called and window.location assigned */ });
it("hides purchase buttons when prices are unavailable", async () => { /* prices_available:false; no upgrade buttons */ });
it("shows the success toast when returning from checkout", async () => { /* initial route /settings/billing?checkout=success; toast text visible */ });
```

  Escreva os 5 testes por extenso (os comentários acima são o roteiro do
  conteúdo, não placeholders a deixar no código): cada um monta o mock de
  fetch roteado, renderiza e afirma o resultado. Para o redirect do checkout,
  substitua `window.location.assign` por spy (`vi.spyOn`) em vez de navegar.

- [ ] **Step 2: rodar e ver falhar** — `cd web && npm run test:ee -- Billing`.
- [ ] **Step 3: hooks em `queries.ts`** (padrão dos existentes
  `useInvites`/`useCreateInvite`): `useBillingCatalog` (`queryKey:
  ["billing-catalog"]`, `staleTime: 12 * 60 * 60 * 1000` — o cache longo do
  lado do painel pedido pelo usuário), `useStartCheckout` e `useOpenPortal`
  (mutations).
- [ ] **Step 4: o componente.** Estrutura (usar os tokens/idioma do DS,
  eyebrow mono, `animate-rise`):
  - `useSearchParams` para `checkout` (success/cancel → toast uma vez, depois
    limpa o param) e `highlight`.
  - Header: `PageHeader title={t("billing.title")} subtitle={t("billing.subtitle")}`.
  - Controles: toggle ciclo (Monthly/Yearly, selo "2 meses grátis" no anual) e
    toggle moeda USD/BRL (escondido quando `currency_locked`; quando travada,
    a moeda exibida é a travada).
  - Grid responsivo (`grid gap-4 md:grid-cols-2 xl:grid-cols-5`) de Cards:
    nome do plano (font-heading), preço do ciclo/moeda ativos
    (`text-stat font-heading`; Free mostra `$0`; Custom mostra
    `t("billing.custom")` e o mailto `contato@quarkus.com.br`), lista de
    limites (formatados: `null` → `t("billing.unlimited")`; números com
    `Intl.NumberFormat`), features com check. Card do plano atual:
    `border-accent-line` + selo `t("billing.currentPlan")`; card em
    `highlight`: mesmo destaque + `scroll_to`/`ref.scrollIntoView` on mount.
  - Botão por card pago: Owner → `Button` que chama `startCheckout` e
    `window.location.assign(url)`; erro 409 (`ApiError.status === 409`) →
    troca um estado `hasActiveSubscription` que transforma TODOS os botões
    pagos em `t("billing.managePortal")` chamando `openPortal` +
    `window.location.assign(url)`; não-Owner → botão `disabled` com
    `title={t("billing.ownerOnly")}`.
  - `prices_available === false` → sem botões nem preços, só a grade (e uma
    linha `text-muted-foreground` explicando que o billing não está
    configurado).
  - Loading: Skeleton no formato da grade; erro: Card destructive como o
    Members faz.
- [ ] **Step 5: strings desta tela** — estenda o namespace `billing:` criado
  na Task 3, nos DOIS arquivos i18n com a mesma shape, com todas as chaves que
  o componente usa: `title`, `subtitle`, `currentPlan`, `unlimited`, `custom`,
  `contactUs`, `upgrade`, `managePortal`, `ownerOnly`, `monthly`, `yearly`,
  `yearlyBadge`, `checkoutSuccess`, `checkoutCanceled`, `notConfigured`,
  `limits.domains`, `limits.members`, `limits.automation`, `limits.clicks`,
  `limits.retention` (EN direto, PT natural).
- [ ] **Step 6: rota** em `web/src/ee/index.tsx`:
  `const Billing = lazy(() => import("./Billing").then((m) => ({ default: m.Billing })));`
  e `{ path: "settings/billing", element: suspended(<Billing />) }` em
  `eeRoutes`.
- [ ] **Step 7: rodar test:ee + lint + typecheck; commit**
  `feat(web/ee): tela de billing com a grade de planos (LUC-41)`.

---

### Task 5: i18n, login e docs

**Files:**
- Modify: `web/src/i18n/en.ts`, `web/src/i18n/pt-BR.ts`,
  `web/src/routes/Login.tsx`, `docs/BILLING.md`, `docs/BILLING.PT_BR.md`
- Test: teste novo no arquivo de teste do Login (ache `Login.test.tsx`; se não
  existir, crie ao lado seguindo o padrão dos outros testes de rota core)

**Interfaces:**
- Produces: chave `login.memberLimit` nos dois arquivos i18n (o namespace
  `billing:` já nasceu na Task 3 e foi estendido na Task 4) e a seção de docs.

- [ ] **Step 1: teste do login que falha** — renderiza `Login` com router em
  `/login?error=member_limit_reached` e espera o texto de
  `login.memberLimit` ("This workspace is at its plan's member limit. Ask the
  workspace admin to upgrade." / PT equivalente natural).
- [ ] **Step 2: implementar** — em `Login.tsx`, junto do `useSearchParams`
  existente: `const callbackError = params.get("error");` e um bloco
  `role="alert"` com a mensagem quando `callbackError === "member_limit_reached"`
  (padrão visual do erro de token existente).
- [ ] **Step 3: strings** — `login.memberLimit` nos dois arquivos i18n, mesma
  shape (o typecheck prova).
- [ ] **Step 4: docs** — em `docs/BILLING.md` e twin: seção curta "The panel
  screen" descrevendo `/settings/billing` (grade, quem vê, quem compra,
  moeda travada, portal) e o toast de limite; mencionar que sem as env de
  billing a tela fica informativa. Prosa direta, sem em-dashes.
- [ ] **Step 5: suites completas do painel** (`npm test`, `npm run test:ee`,
  lint, typecheck) e gates Rust (fmt/clippy dois modos + billing_it/plan_it);
  commit `feat(web): i18n do billing, erro de login e docs (LUC-41)`.

---

### Task 6: verificação final e PR

- [ ] Suites completas: Rust nos dois modos (com Postgres) e painel
  (`npm test` + `npm run test:ee`), lint e typecheck.
- [ ] Prova open-core: `rm -rf src/ee web/src/ee` → `cargo build` +
  `cd web && npm run build` (o stub cobre o painel) → restaurar com
  `git checkout -- src/ee web/src/ee`.
- [ ] Smoke visual: `cd web && npm run dev` apontando pro backend local com
  billing de teste (ou mock), abrir `/settings/billing`, conferir grade nos
  dois temas e no mobile (grid colapsa pra 1-2 colunas).
- [ ] Push da branch e PR pra `main` com o resumo das 5 tasks, checklist de
  testes e screenshot da tela.

## Fora deste plano

Página pública de preços (LUC-40). Fase 3. Mudanças na grade. Telas EE no
design system publicado do claude.ai/design (decisão em aberto).
