# LUC-94 — Follow-ups do design refresh — plano de implementação

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** liquidar os 13 follow-ups da LUC-94 (cosméticos/DRY/a11y do design refresh) sem mudar comportamento além das 3 decisões de produto aprovadas.

**Architecture:** três tasks independentes por área (mecânicos+i18n, DRY, UX), cada uma com testes atualizados junto. Spec: `docs/specs/2026-07-24-luc94-followups-design.md`.

**Tech Stack:** a mesma do painel (React 19, Tailwind 4, Vitest, oxlint, TS 7).

## Global Constraints

- Branch: `chore/luc94-design-refresh-followups`. Nunca commitar na main.
- Gate por task: `cd web && npm run lint && npm run typecheck && npm test`.
- Comportamento inalterado, exceto: chips com cap+"+N", botão do PageHeader removido, avatares chart-2/3.
- Tokens only; i18n nas 2 locales com shapes idênticas; testes nunca enfraquecidos.
- Grep de segurança antes de remover qualquer i18n key (zero referências).

---

### Task 1: Mecânicos + i18n

**Files:**
- Modify: `web/src/i18n/en.ts`, `web/src/i18n/pt-BR.ts` (remoção de keys mortas + rename), `web/src/routes/ExtensionDetail.tsx` e/ou `Extensions.tsx` (call site do rename), `web/src/components/Terminal.tsx` (+ test), `web/src/index.css` (webkit-mask), `web/src/components/StatsCharts.tsx` (maxBarSize), `web/src/routes/Webhooks.tsx` (+ test, accname)

**Interfaces:**
- Produces: nada consumido pelas outras tasks (independente).

- [ ] **Step 1: i18n sweep.** Para CADA key da lista da spec: `grep -rn "<key>" web/src --include="*.ts*"` — se só a definição aparecer (en+pt-BR), remover das duas locales. Registrar no report as que tinham referência e ficaram.
- [ ] **Step 2: rename.** `extensions.backAria` → `extensions.backToExtensions` nas 2 locales + call sites (grep). Valor mantido.
- [ ] **Step 3: Terminal a11y.** Nos 3 spans traffic-light: `aria-hidden="true"`. Teste do Terminal: os dots continuam com `data-testid`; adicionar assert de `aria-hidden`.
- [ ] **Step 4: webkit-mask.** No `@utility bg-dot-grid` do `index.css`, adicionar `-webkit-mask-image:` com o mesmo valor do `mask-image` existente.
- [ ] **Step 5: maxBarSize.** No BarChart de cliques/dia (`StatsCharts.tsx`), `maxBarSize={48}` no `<Bar>`.
- [ ] **Step 6: Switch accname.** Em `Webhooks.tsx`, o `aria-label` do Switch vira o texto visível primeiro: `` `${t("webhooks.columnActive")} — ${webhook.url}` `` (uma expressão para ativo/inativo — o estado vem do `aria-checked`). Ajustar o teste do toggle que busca pelo aria-label antigo (mesma intenção: achar o switch do webhook certo).
- [ ] **Step 7: gate + commit** `chore(web): i18n sweep, a11y do terminal, accname do switch, webkit-mask e maxBarSize (LUC-94)`

### Task 2: DRY

**Files:**
- Modify: `web/src/lib/utils.ts` (FIELD_LABEL_CLASS), `web/src/components/CreateTokenDialog.tsx`, `web/src/components/CreateWorkspaceForm.tsx`, `web/src/routes/OidcProvider.tsx`, `web/src/routes/SsoDomains.tsx` (importam)
- Create: `web/src/components/OutOfShellFrame.tsx` + `OutOfShellFrame.test.tsx`
- Modify: `web/src/routes/Login.tsx`, `web/src/routes/Onboarding.tsx`, `web/src/routes/AcceptInvite.tsx` (migram para o frame)
- Modify: `web/src/lib/short-url.ts` (+ test), `web/src/components/LinkTable.tsx` (buildShortUrl deriva)

**Interfaces:**
- Produces: `export const FIELD_LABEL_CLASS = "text-[13px] font-normal text-muted-foreground"` em `@/lib/utils`; `export function OutOfShellFrame(props: { title: ReactNode; subtitle?: ReactNode; topRight?: ReactNode; children: ReactNode })`; `resolveShortHost` permanece com a mesma assinatura.

- [ ] **Step 1: FIELD_LABEL_CLASS.** Export em `lib/utils.ts`; os 4 arquivos trocam a const local pelo import. Grep final: 1 definição.
- [ ] **Step 2 (TDD): OutOfShellFrame.** Teste primeiro (smoke: title em h1, subtitle, children renderizam, backdrop aria-hidden presente, topRight renderiza). Implementar extraindo o markup EXATO compartilhado por Login/Onboarding/AcceptInvite (backdrop 2 camadas, coluna central, glifo `glow-glyph`, h1 26px display, sub muted). Migrar as 3 telas preservando diferenças (Login: topRight=LanguageSwitcher; AcceptInvite: 3 estados usam o mesmo frame). Suites das 3 telas continuam verdes sem enfraquecer.
- [ ] **Step 3: fonte única do host.** Em `LinkTable.tsx`, `buildShortUrl` passa a chamar `resolveShortHost` (de `@/lib/short-url`) e aplicar o protocolo/prefixo como hoje. Nenhum valor de saída muda (os testes de LinkTable seguram: copy/QR). Se a assinatura atual de `resolveShortHost` não servir direto, adaptar em `short-url.ts` mantendo os 4 unit tests de precedência.
- [ ] **Step 4: gate + commit** `refactor(web): FIELD_LABEL_CLASS unica, OutOfShellFrame compartilhado e fonte unica do short host (LUC-94)`

### Task 3: UX decididos

**Files:**
- Modify: `web/src/routes/Links.tsx` (+ test: chips cap, botão removido), `web/src/routes/Members.tsx` (+ test: avatares), `web/src/index.css` + `web/src/components/ui/dialog.tsx` + `alert-dialog.tsx` (exit), i18n (2 locales: label do chip "+N"/"menos")

**Interfaces:**
- Consumes: nada das outras tasks.

- [ ] **Step 1 (TDD): chips cap.** Teste: fixture com 14 tags → renderiza 10 chips + botão `+4`; clicar expande para 14 + botão de recolher; toda tag continua clicável ao expandir. Implementar com estado local (`showAllTags`), chip-botão com estilo inativo (`border-border text-muted-foreground`), i18n `links.moreTags`/`links.lessTags` ("+{count}" / "Show less"/"Mostrar menos").
- [ ] **Step 2: botão do PageHeader.** Remover o `actions` de criar do PageHeader do Links (o dialog continua montado; topbar `?new=1` e empty-state CTA intactos). Ajustar testes que clicavam nesse botão (usar o CTA do empty state ou `?new=1`, mantendo a cobertura do fluxo de criação).
- [ ] **Step 3: avatares.** `AVATAR_HUES` = `["var(--chart-2)", "var(--chart-3)"]` em `Members.tsx`, comentário atualizado; teste de rotação `[2-4]`→`[2-3]`.
- [ ] **Step 4: exit do dialog.** `index.css`: `@utility animate-rise-out { animation: rise-out 0.15s cubic-bezier(0.23, 1, 0.32, 1) both; }` + `@keyframes rise-out { from { opacity: 1; transform: none; } to { opacity: 0; transform: translateY(14px); } }`. Em `dialog.tsx` e `alert-dialog.tsx`, content ganha `data-closed:animate-rise-out` (mantendo `animate-rise` na entrada — se conflitarem na mesma propriedade, escopo a entrada com `data-open:`).
- [ ] **Step 5: gate + commit** `feat(web): chips com cap expansivel, botao unico de criar, avatares neutros e exit do dialog (LUC-94)`

---

## Self-review do plano (feito)

Cobertura: 13/13 itens da issue mapeados (T1: 6 · T2: 3 · T3: 4). Sem placeholders; interfaces declaradas; decisões de produto refletidas.
