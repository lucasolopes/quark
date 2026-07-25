# LUC-96 — Painel responsivo — plano de implementação

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** painel funcional em 360/768/1024/1440px conforme a spec `docs/specs/2026-07-24-luc96-responsive-design.md`.

**Architecture:** drawer mobile sobre o Dialog do Base UI com fonte única de nav; dialog full-screen por prop; reflow por classes responsivas; QA de scroll-x automatizado.

**Tech Stack:** a do painel (React 19, Tailwind 4, Base UI, Vitest, Playwright local).

## Global Constraints

- Branch: `chore/luc96-responsive`. Gate por task: `cd web && npm run lint && npm run typecheck && npm test` (`--maxWorkers=4` aceito contra flakes de máquina).
- Desktop (≥ 768px) fica VISUALMENTE IDÊNTICO ao atual — mudanças só abaixo dos cortes (exceção: remoção do trilho `w-16`, que só existia em < 640).
- Comportamento/APIs intactos; tokens only; i18n nas 2 locales; testes nunca enfraquecidos.
- Fonte única dos itens de nav (zero duplicação da lista entre sidebar e drawer).

---

### Task 1: Shell responsivo — drawer + topbar mobile

**Files:**
- Create: `web/src/components/MobileNav.tsx` + `MobileNav.test.tsx`
- Modify: `web/src/app/Shell.tsx` (+ `Shell.test.tsx`), `web/src/i18n/en.ts`, `pt-BR.ts`

**Interfaces:**
- Produces: `export function MobileNav(props: { open: boolean; onOpenChange: (open: boolean) => void; groups: NavGroup[]; footer?: ReactNode; children?: ReactNode })` — onde `NavGroup` é o shape que o Shell já monta (`{ label, items: { to, label, icon, show }[] }`) exportado do Shell ou de um módulo comum. O Shell é o dono do estado `mobileNavOpen`.

- [ ] **Step 1 (TDD):** testes primeiro — MobileNav renderiza grupos/itens com labels; clicar num item chama `onOpenChange(false)` (fecha ao navegar) e navega; Esc fecha; footer renderiza. Shell.test: em viewport estreito (mock `matchMedia`? NÃO — o drawer/hambúrguer renderizam sempre no DOM com classes `md:hidden`; testar por presença/interação, não por viewport): hambúrguer presente, abre o drawer, item de nav navega e fecha; lupa expande a busca (input aparece com autofocus) e recolhe.
- [ ] **Step 2:** extrair os `navGroups` para constante/hook reutilizável no próprio Shell e implementar o `MobileNav` sobre o Dialog do Base UI (painel esquerdo 280px `bg-sidebar`, slide-in `data-open:`/`data-closed:` com utilities novas `animate-slide-in-left`/`out` no index.css seguindo o padrão rise/rise-out; itens de nav com alvo ≥44px `min-h-11`).
- [ ] **Step 3:** topbar responsiva no Shell: `md:hidden` para hambúrguer/lupa/+; `hidden md:flex` para busca inline/lang/tema/botão completo; estado `searchExpanded` (< md) com input em linha própria (mesma lógica live/`?q=` — reutilizar o handler existente); LanguageSwitcher + toggle de tema no footer do drawer. Remover o trilho `w-16` (sidebar vira `hidden md:flex` com 250px fixo).
- [ ] **Step 4:** i18n novas keys (hambúrguer aria, lupa aria, fechar busca) nas 2 locales. Gate + commit `feat(web): drawer de navegacao mobile e topbar responsiva (LUC-96)`.

### Task 2: Dialog full-screen mobile

**Files:**
- Modify: `web/src/components/ui/dialog.tsx`, `web/src/components/CreateLinkDialog.tsx`, `web/src/components/EditLinkDialog.tsx` (+ testes dos três)

- [ ] **Step 1 (TDD):** teste do DialogContent com `fullScreenOnMobile`: classes mobile (`max-sm:...` inset/h-dvh/rounded-none) presentes; sem a prop, ausentes. Testes dos dois dialogs asserting a prop aplicada.
- [ ] **Step 2:** implementar a prop no `DialogContent` (classes condicionais `max-sm:inset-0 max-sm:h-dvh max-sm:max-w-none max-sm:rounded-none max-sm:m-0`; corpo scrollável; `DialogFooter` sticky no modo full: `max-sm:sticky max-sm:bottom-0 max-sm:bg-card max-sm:border-t` — ajustar os negativos `-mx-6 -mb-6` para não vazar). Aplicar `fullScreenOnMobile` no Create e Edit.
- [ ] **Step 3:** gate + commit `feat(web): dialogs grandes full-screen no mobile (LUC-96)`.

### Task 3: Reflow das telas

**Files:**
- Modify: `web/src/components/LinkTable.tsx` (+ test), `web/src/routes/Links.tsx`, `StatsView.tsx`/`StatsCharts.tsx`/`RecentEventsTable.tsx`, `Domains.tsx`, `Members.tsx`, `Tokens.tsx`, `Webhooks.tsx`, `Pixels.tsx`, `Import.tsx`, `Extensions.tsx`, `ExtensionDetail.tsx`, `SsoDomains.tsx`, `OidcProvider.tsx`, `AppLinks.tsx`, `OutOfShellFrame.tsx` (paddings)

- [ ] **Step 1:** card de link reflui em < sm: bloco principal full-width; rodapé do card com cliques + ações em linha própria (`flex-wrap`/reordenação com classes responsivas; NADA muda ≥ sm). Teste: os mesmos elementos continuam presentes/acionáveis.
- [ ] **Step 2:** filter row e bulk bar do Links com `flex-wrap`/empilhamento < sm; KPI grid do stats validado (`grid-cols-2 lg:grid-cols-4`); `RecentEventsTable` e tabela de resultados do Import com `overflow-x-auto` no container; grids de settings/extensions para `grid-cols-1 sm:grid-cols-2 xl:grid-cols-3`; rows de settings com `flex-wrap` para ações caírem de linha; `OutOfShellFrame` paddings mobile (`p-4`+`py-8`).
- [ ] **Step 3:** touch targets < sm nos controles principais (ações de card `size-11` de alvo via padding responsivo OU wrapper com `min-h-11 min-w-11` — visual inalterado ≥ sm).
- [ ] **Step 4:** gate + commit `feat(web): reflow responsivo das telas (LUC-96)`.

### Task 4: QA responsivo automatizado + varredura

**Files:**
- Create: `web/scripts/responsive-qa.mjs` (script local, NÃO entra no CI)
- Modify: o que a varredura apontar

- [ ] **Step 1:** script Playwright standalone (padrão do `depara.mjs` desta sessão: backend debug local + vite dev + login com token dev): percorre TODAS as rotas autenticadas + /login em 360/768/1024/1440 × dark/light; para cada combinação: (a) screenshot em `scratchpad`, (b) FALHA se `document.documentElement.scrollWidth > window.innerWidth + 1`.
- [ ] **Step 2:** rodar, corrigir todo scroll-x/quebra que aparecer (iterar até zero falhas), registrar no report as correções.
- [ ] **Step 3:** gate completo + commit `test(web): qa responsivo automatizado e ajustes finais (LUC-96)`.

---

## Self-review do plano (feito)

Cobre as 4 áreas da spec; decisões de produto refletidas (drawer md, topbar enxuta, full-screen sm); QA automatizado de scroll-x incluído; desktop intocado como constraint global.
