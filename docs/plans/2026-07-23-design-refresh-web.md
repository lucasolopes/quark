# Repaginada do painel web (Quark DS v2) — plano de implementação

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** aplicar o design system Quark DS v2 (mock em `docs/research/design-refresh/`) em todo o painel `web/` (login, shell, 14 telas) mais a passada de engenharia: TypeScript 7, Google TS Style Guide, `.npmrc` e fontes self-hosted.

**Architecture:** restyle profundo sobre a base existente (React 19 + Vite 8 + Tailwind 4 + shadcn/Base UI). Tokens v2 entram no `web/src/index.css` mapeados nas variáveis shadcn; variants CVA dos `ui/*` são ajustadas; componentes novos (StatCard, MeterBar, Terminal, PageHeader) nascem em `src/components/`; cada tela é refeita visualmente preservando lógica, queries, i18n e RBAC.

**Tech Stack:** TypeScript 7, Tailwind CSS 4 (`@theme`), class-variance-authority, @fontsource-variable, lucide-react, TanStack Query/Table, react-router 7, recharts, Vitest + Testing Library, oxlint.

**Spec:** `docs/specs/2026-07-23-design-refresh-web-design.md`

## Restrições globais

- Branch de trabalho: `feat/design-refresh` (criada na Task 1). NUNCA commitar na `main` (auto-deploy).
- Todo trabalho acontece em `web/` (exceto docs). Comandos rodam com `cd web`.
- Gate de cada task: `npm run lint && npm run typecheck && npm test` verdes (build nas tasks que mexem em toolchain: `npm run build`).
- Comportamento intacto: nenhuma mudança de API, rota ou fluxo, exceto os dois aditivos aprovados na spec (busca global da topbar → `/links?q=`; botão global "Novo link" na topbar).
- Cores/efeitos SEMPRE via tokens (variáveis CSS/classes de tema). Proibido hex solto em classe utilitária de tela (ex.: `bg-[#131521]`). Exceções permitidas: os SVGs de marca e as cores dos traffic-lights do Terminal.
- Strings novas de UI entram em `src/i18n/en.ts` E `src/i18n/pt-BR.ts` (mesma shape; `en` é a fonte da verdade).
- Textos seguem a voz do DS (`docs/research/design-refresh/design-system-readme.md`): sentence-case, eyebrows UPPERCASE mono, sem emoji.
- Testes: atualizar os `*.test.tsx` afetados na MESMA task; componentes novos nascem com teste (TDD).
- Referência visual: `docs/research/design-refresh/mock/sections/*.html`. Em dúvida, o mock manda.

## Referência de design (todas as tasks de tela consomem)

### Mapeamento paleta do mock → tokens

O mock usa `pal.*` (definida em `mock/sections/_script0.js` linhas 242‑244). Mapeamento canônico para o código:

| Mock (`pal.*`) | Dark | Light | Token CSS | Classe Tailwind |
|---|---|---|---|---|
| `bg` | `#0A0B0F` | `#EEF0F4` | `--background` | `bg-background` |
| `ink2` (sidebar) | `#0C0D13` | `#E7EAF0` | `--sidebar` | `bg-sidebar` |
| `card` | `#131521` | `#FFFFFF` | `--card` | `bg-card` |
| `card2` (nested) | `#1A1D2B` | `#F4F5F8` | `--secondary` | `bg-secondary` |
| `text` | `#E8EAF2` | `#12141C` | `--foreground` | `text-foreground` |
| `strong` (títulos) | `#F3F5FA` | `#12141C` | `--text-strong` (novo) | `text-strong` |
| `muted` | `#8A90A2` | `#5E6472` | `--muted-foreground` | `text-muted-foreground` |
| `border` | `rgba(255,255,255,.09)` | `rgba(0,0,0,.10)` | `--border` | `border-border` |
| `borderStrong` | `rgba(255,255,255,.16)` | `rgba(0,0,0,.16)` | `--input` | `border-input` |
| `brand` (texto lime) | `#C6F94E` | `#4A7A17` | `--brand-ink` | `text-brand-ink` |
| `fill` (fill lime) | `#C6F94E` | `#8FD12E` | `--primary` | `bg-primary` |
| `wash` | `rgba(198,249,78,.12)` | `rgba(143,209,46,.14)` | `--accent-wash` (novo) | `bg-accent-wash` |
| `input` (wells) | `#0F1119` | `#FFFFFF` | `--surface-input` (novo) | `bg-surface-input` |
| `hover` | `rgba(255,255,255,.04)` | `rgba(0,0,0,.04)` | `--surface-hover` (novo) | `bg-surface-hover` |
| `shadow` | `0 1px 2px rgba(0,0,0,.4)` | `0 1px 2px rgba(0,0,0,.08)` | `--shadow-card` (novo) | `shadow-card` |

### Receitas recorrentes do mock

- **Título de página:** `<h1 className="font-heading text-page-title font-bold tracking-display text-strong">` + subtítulo `text-[13.5px] text-muted-foreground mt-1`. Encapsulado no `PageHeader` (Task 6) — telas usam o componente.
- **Card de lista (rows tipo Links/Domains/Tokens/Webhooks):** `rounded-lg border border-border bg-card shadow-card p-4` + hover lift via classe `card-hover` (utility da Task 4).
- **Ícone em well:** quadrado `size-10 rounded-[9px] bg-accent-wash border border-accent-line flex items-center justify-center` com ícone lucide `size-[18px] text-brand-ink` (stroke padrão do lucide ≈ 2, ok vs 1.9 do mock).
- **Código/short mono:** `font-mono text-[14.5px] font-medium text-brand-ink`.
- **Numeral de métrica:** `font-heading font-bold text-strong` com `font-feature-settings: 'tnum'` (o `.font-heading` global já aplica tnum).
- **Chip/pill:** `rounded-full border px-3 py-1.5 text-[13px]`; ativo = `bg-accent-wash border-accent-chip text-brand-ink`, inativo = `border-border text-muted-foreground`.
- **Status dot:** `size-2 rounded-full` + cor semântica (`bg-primary` ok, `bg-destructive` erro, `bg-[--cyan]`? NÃO — usar `bg-chart-2`).
- **Botão primário (fill lime):** componente `Button` variant `default` (Task 5) — `font-bold`, hover clareia.
- **Ações quadradas 32px (copy/stats/menu):** `Button` variant `outline` size `icon`.
- **Entrada de página:** `animate-rise` (utility da Task 4) no container raiz da tela.

### Ícones (lucide)

Sidebar/telas mantêm os ícones lucide atuais. Correspondências do mock: link → `Link2`, analytics → `BarChart3`, domains → `Globe`, members → `Users`, tokens → `KeyRound` (mock desenha uma chave), webhooks → `Zap` no mock, mas o app usa `Webhook` — manter `Webhook`.

---

### Task 1: Branch, `.npmrc`, `engines` e TypeScript 7

**Files:**
- Create: `web/.npmrc`
- Modify: `web/package.json`

**Interfaces:**
- Produces: branch `feat/design-refresh`; toolchain TS 7 funcionando (`npm run typecheck`, `npm run build`) para todas as tasks seguintes.

- [ ] **Step 1: Criar a branch**

```bash
git checkout -b feat/design-refresh
```

- [ ] **Step 2: Criar `web/.npmrc`**

```ini
# Instalações reprodutíveis: versões exatas no package.json.
save-exact=true
# Falha cedo quando o Node local não casa com "engines".
engine-strict=true
# Sem banner de funding no CI/terminal.
fund=false
```

- [ ] **Step 3: Adicionar `engines` ao `web/package.json`**

Logo após `"type": "module",`:

```json
  "engines": {
    "node": ">=20"
  },
```

(`.node-version` é `20` — o engine-strict passa a validar isso.)

- [ ] **Step 4: Upgrade TypeScript 7**

```bash
cd web && npm install -D typescript@7
```

Se o npm não resolver `typescript@7` (improvável — GA em 2026-07-08), tentar `typescript@next`. Anotar a versão instalada.

- [ ] **Step 5: Verificar toolchain completa**

```bash
cd web && npm run typecheck && npm run build && npm test
```

Expected: tudo verde. O `tsc -b` (project references + `noEmit`) é o modo usado também no build do CF Pages.

**FALLBACK (só se o Step 5 falhar por incompatibilidade do TS 7 com `tsc -b`/project references):** reverter para `npm install -D typescript@~6.0.2 --save-exact`, registrar o erro exato num comentário no topo do `web/tsconfig.json` (`/* TS7 bloqueado por: <erro>. Reavaliar no 7.1. */`) e seguir o plano — as demais tasks não dependem do TS 7.

- [ ] **Step 6: Commit**

```bash
git add web/.npmrc web/package.json web/package-lock.json
git commit -m "chore(web): .npmrc com boas praticas, engines e TypeScript 7"
```

---

### Task 2: Google TS Style Guide no oxlint + correções

**Files:**
- Modify: `web/.oxlintrc.json`
- Modify: `web/src/app/App.tsx`, `web/src/main.tsx` (default export → named)
- Modify: quaisquer arquivos que o lint novo apontar

**Interfaces:**
- Produces: `npm run lint` passa com o ruleset novo; código segue o guide (imports de tipo explícitos, sem default export em `src/`, sem `any` novo).

- [ ] **Step 1: Substituir `web/.oxlintrc.json`**

```json
{
  "$schema": "./node_modules/oxlint/configuration_schema.json",
  "plugins": ["react", "typescript", "oxc", "import", "unicorn"],
  "categories": {
    "correctness": "error",
    "suspicious": "warn"
  },
  "rules": {
    "react/rules-of-hooks": "error",
    "react/only-export-components": ["warn", { "allowConstantExport": true }],
    "typescript/consistent-type-imports": "error",
    "typescript/no-explicit-any": "error",
    "typescript/array-type": ["error", { "default": "array-simple" }],
    "typescript/no-non-null-assertion": "warn",
    "import/no-default-export": "error",
    "eqeqeq": ["error", "smart"],
    "no-var": "error",
    "prefer-const": "error",
    "no-throw-literal": "error",
    "unicorn/prefer-node-protocol": "error"
  },
  "overrides": [
    {
      "files": ["src/components/ui/**"],
      "rules": {
        "react/only-export-components": "off"
      }
    },
    {
      "files": ["src/app/router.tsx"],
      "rules": {
        "react/only-export-components": "off"
      }
    },
    {
      "files": ["vite.config.ts", "playwright.config.ts", "e2e/**"],
      "rules": {
        "import/no-default-export": "off"
      }
    }
  ]
}
```

(Racional Google TS Style Guide: named exports sempre; `import type` para tipos — o `verbatimModuleSyntax` do tsconfig já força no compilador e a regra alinha o lint; `Array<T>` só para tipos complexos; sem `any`.)

- [ ] **Step 2: Rodar o lint e listar violações**

```bash
cd web && npm run lint
```

Expected: FALHA apontando pelo menos o default export de `src/app/App.tsx`.

- [ ] **Step 3: Corrigir `App.tsx` para named export**

Em `web/src/app/App.tsx`: trocar `export default App` (ou `export default function App`) por `export function App() { ... }`. Em `web/src/main.tsx`: `import { App } from './app/App.tsx'`.

- [ ] **Step 4: Corrigir as demais violações**

Corrigir cada apontamento do lint (imports de tipo, `any`, etc.) SEM suprimir regras. Se alguma regra do Step 1 não existir na versão do oxlint instalada (erro "unknown rule"), removê-la do config e registrar no commit message quais saíram.

- [ ] **Step 5: Verificar**

```bash
cd web && npm run lint && npm run typecheck && npm test
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "style(web): Google TS Style Guide via oxlint (named exports, type imports, sem any)"
```

---

### Task 3: Fontes self-hosted via @fontsource

**Files:**
- Modify: `web/package.json` (adicionar 3 @fontsource-variable, remover geist), `web/src/main.tsx`, `web/index.html`, `web/src/index.css`

**Interfaces:**
- Produces: fontes bundladas; famílias CSS `'Space Grotesk Variable'`, `'Hanken Grotesk Variable'`, `'JetBrains Mono Variable'` disponíveis.

- [ ] **Step 1: Trocar pacotes**

```bash
cd web && npm uninstall @fontsource-variable/geist && npm install @fontsource-variable/space-grotesk @fontsource-variable/hanken-grotesk @fontsource-variable/jetbrains-mono
```

- [ ] **Step 2: Importar no `web/src/main.tsx`** (antes de `./index.css`)

```tsx
import '@fontsource-variable/space-grotesk'
import '@fontsource-variable/hanken-grotesk'
import '@fontsource-variable/jetbrains-mono'
import './index.css'
```

- [ ] **Step 3: Remover o CDN do `web/index.html`**

Apagar as 3 linhas `<link rel="preconnect" ...>` / `<link href="https://fonts.googleapis.com/...">`.

- [ ] **Step 4: Atualizar as famílias no `web/src/index.css`**

No bloco `@theme inline`:

```css
  --font-sans: 'Hanken Grotesk Variable', 'Hanken Grotesk', system-ui, sans-serif;
  --font-heading: 'Space Grotesk Variable', 'Space Grotesk', system-ui, sans-serif;
  --font-mono: 'JetBrains Mono Variable', 'JetBrains Mono', ui-monospace, monospace;
```

- [ ] **Step 5: Verificar (fonte no bundle, zero request externo)**

```bash
cd web && npm run build && grep -ri "googleapis" dist/ | wc -l
```

Expected: build PASS e `0` ocorrências. Abrir `npm run dev` e conferir visualmente que os títulos continuam em Space Grotesk (DevTools → Computed → font-family) se houver dúvida.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat(web): fontes self-hosted via @fontsource-variable (remove CDN e geist)"
```

---

### Task 4: Tokens v2 no `index.css`

**Files:**
- Modify: `web/src/index.css`
- Test: os testes existentes seguem passando (`npm test`)

**Interfaces:**
- Produces: tokens/classes usados por TODAS as tasks seguintes: `text-strong`, `bg-accent-wash`, `border-accent-line`, `border-accent-chip`, `bg-surface-input`, `bg-surface-hover`, `shadow-card`, `shadow-modal`, `text-page-title`, `text-stat`, `text-subtitle`, `tracking-display`, `animate-rise`, `card-hover`, `glow-accent` (já existe).

- [ ] **Step 1: Ajustar o radius base**

Em `:root`, trocar `--radius: 0.875rem;` por:

```css
  --radius: 0.75rem; /* 12px — cards 12, xl(modal) ≈ 17, md(botões/inputs) ≈ 10, sm(chips) ≈ 7 (escala do mock v2) */
```

- [ ] **Step 2: Adicionar os tokens novos por tema**

No fim do bloco `:root` (light):

```css
  --text-strong: #12141c;
  --accent-wash: rgba(143, 209, 46, 0.14);
  --accent-line: rgba(143, 209, 46, 0.28);
  --accent-chip: rgba(143, 209, 46, 0.4);
  --surface-input: #ffffff;
  --surface-hover: rgba(0, 0, 0, 0.04);
  --shadow-card: 0 1px 2px rgba(0, 0, 0, 0.08);
  --shadow-modal: 0 40px 90px -30px rgba(0, 0, 0, 0.35);
  --sidebar: #e7eaf0; /* substitui o #ffffff atual: ink2 light do mock */
```

(a linha `--sidebar` SUBSTITUI o valor existente no bloco light, não duplica)

No fim do bloco `.dark`:

```css
  --text-strong: #f3f5fa;
  --accent-wash: rgba(198, 249, 78, 0.12);
  --accent-line: rgba(198, 249, 78, 0.28);
  --accent-chip: rgba(198, 249, 78, 0.4);
  --surface-input: #0f1119;
  --surface-hover: rgba(255, 255, 255, 0.04);
  --shadow-card: 0 1px 2px rgba(0, 0, 0, 0.4);
  --shadow-modal: 0 40px 90px -30px rgba(0, 0, 0, 0.7);
```

- [ ] **Step 3: Expor no `@theme inline`**

Adicionar dentro do bloco `@theme inline` existente:

```css
  --color-strong: var(--text-strong);
  --color-accent-wash: var(--accent-wash);
  --color-accent-line: var(--accent-line);
  --color-accent-chip: var(--accent-chip);
  --color-surface-input: var(--surface-input);
  --color-surface-hover: var(--surface-hover);
  --shadow-card: var(--shadow-card);
  --shadow-modal: var(--shadow-modal);
  --text-page-title: 27px;
  --text-page-title--line-height: 1.15;
  --text-stat: 30px;
  --text-stat--line-height: 1.1;
  --text-subtitle: 13.5px;
  --text-subtitle--line-height: 1.5;
  --tracking-display: -0.03em;
```

- [ ] **Step 4: Utilities de motion/hover no fim do arquivo**

```css
/* Entrada padrão de página/modal do DS (qrise do mock). */
@utility animate-rise {
  animation: rise 0.5s cubic-bezier(0.23, 1, 0.32, 1) both;
}
@keyframes rise {
  from {
    opacity: 0;
    transform: translateY(14px);
  }
  to {
    opacity: 1;
    transform: none;
  }
}

/* Hover lift de card interativo (qcard-hov do mock). */
@utility card-hover {
  transition:
    transform 0.18s cubic-bezier(0.23, 1, 0.32, 1),
    border-color 0.18s ease;
  &:hover {
    transform: translateY(-3px);
    border-color: var(--accent-line);
  }
}
```

- [ ] **Step 5: Verificar**

```bash
cd web && npm run lint && npm run typecheck && npm test && npm run build
```

Expected: PASS (mudança é aditiva; radius muda levemente os raios em tudo — esperado).

- [ ] **Step 6: Commit**

```bash
git add web/src/index.css && git commit -m "feat(web): tokens Quark DS v2 (washes, surfaces, sombras, tipografia, motion)"
```

---

### Task 5: Variants v2 dos componentes ui/*

**Files:**
- Modify: `web/src/components/ui/button.tsx`, `input.tsx`, `badge.tsx`, `card.tsx`, `table.tsx`, `dialog.tsx`, `tabs.tsx`, `dropdown-menu.tsx`
- Test: rodar a suíte inteira; ajustar testes que assertem classes.

**Interfaces:**
- Consumes: tokens da Task 4.
- Produces: mesmos exports e APIs de hoje (`Button`, `buttonVariants`, `Input`, `Badge`, `Card*`, ...). Nenhuma prop nova; só estilo.

- [ ] **Step 1: Button — dimensões e pesos do mock**

Em `button.tsx`, aplicar estas mudanças no `cva`:

- Base: trocar `text-sm font-medium` por `text-sm font-semibold`; manter o resto.
- `variant.default`: `"bg-primary font-bold text-primary-foreground hover:bg-primary/90 dark:hover:bg-[#D8FF70]"` (hover do mock: lime mais claro no dark; no light o /90 escurece de leve).
- `variant.outline`: trocar para `"border-input bg-transparent hover:bg-surface-hover hover:text-foreground aria-expanded:bg-muted"`.
- `variant.ghost`: `"text-muted-foreground hover:bg-surface-hover hover:text-foreground"`.
- `size.default`: altura `h-9` e padding `px-4` (mock md ≈ 40px: 11px + 14px de fonte + 11px).
- `size.lg`: `h-11 px-6 text-[15px]`.
- Manter os demais sizes/variants como estão.

- [ ] **Step 2: Input — well escuro**

Em `input.tsx`, na string de classes: trocar o fundo/borda atuais por `border-input bg-surface-input` (remover `dark:bg-input/30` se existir) e o radius para `rounded-[10px]`; padding `px-3.5 py-2.5`; fonte `text-sm`.

- [ ] **Step 3: Badge — pills e washes**

Em `badge.tsx`, garantir variants:

- `default` (accent): `bg-accent-wash text-brand-ink border border-accent-line rounded-md font-mono text-[11px]`
- `secondary` (mono/neutral): `border border-input text-muted-foreground bg-transparent font-mono text-[11px]`
- `outline`: manter.
- `destructive`: `bg-destructive/10 text-destructive border border-destructive/30`.

Manter a API (`variant` prop) e exports.

- [ ] **Step 4: Card — hairline + shadow-card**

Em `card.tsx`: trocar `ring-1 ring-foreground/10` por `border border-border shadow-card`; radius `rounded-xl` mantém (≈17px com o novo base... conferir visual: o mock usa 12 nos cards de lista e 16 no modal; o Card shadcn é usado como "painel de formulário" — usar `rounded-2xl`? NÃO: manter `rounded-xl` que com `--radius: 0.75rem` ≈ 16.8px, correto para painéis grandes; os cards de lista das telas usam classes próprias `rounded-lg` = 12px).

- [ ] **Step 5: Table — header nested + hairlines**

Em `table.tsx`: header row com `bg-secondary` (panel-2) e `text-[11px] uppercase tracking-[0.06em] text-muted-foreground font-mono`; células com `border-b border-border`; remover zebra se houver.

- [ ] **Step 6: Dialog — modal do DS**

Em `dialog.tsx`: overlay `bg-black/60 backdrop-blur-[4px]`; content `rounded-2xl border border-input bg-card shadow-modal animate-rise p-6 max-w-[540px]`.

- [ ] **Step 7: Tabs + DropdownMenu**

- `tabs.tsx`: lista com `border-b border-border`; trigger ativo `text-brand-ink border-b-2 border-brand-ink` (hairline underline), inativo `text-muted-foreground`.
- `dropdown-menu.tsx`: content `bg-popover border border-border rounded-[10px] shadow-modal`; item hover `bg-surface-hover`.

- [ ] **Step 8: Verificar suíte + dev visual**

```bash
cd web && npm run lint && npm run typecheck && npm test
```

Expected: PASS. Se algum teste assertar classe antiga (ex. `ring-1`), atualizar o teste JUNTO — verificando que a intenção (ex.: "é um card") continua coberta.

- [ ] **Step 9: Commit**

```bash
git add -A && git commit -m "feat(web): variants v2 dos componentes ui (button, input, badge, card, table, dialog, tabs, dropdown)"
```

---

### Task 6: Componentes novos do DS — PageHeader, StatCard, MeterBar, Terminal

**Files:**
- Create: `web/src/components/PageHeader.tsx` + `PageHeader.test.tsx`
- Create: `web/src/components/StatCard.tsx` + `StatCard.test.tsx`
- Create: `web/src/components/MeterBar.tsx` + `MeterBar.test.tsx`
- Create: `web/src/components/Terminal.tsx` + `Terminal.test.tsx`

**Interfaces:**
- Consumes: tokens Task 4.
- Produces (assinaturas exatas — telas das Tasks 8‑21 importam daqui):

```tsx
export function PageHeader(props: { title: ReactNode; subtitle?: ReactNode; actions?: ReactNode; back?: { label: string; to: string } }): ReactElement
export function StatCard(props: { value: ReactNode; label: ReactNode; accent?: boolean; className?: string }): ReactElement
export function MeterBar(props: { label: ReactNode; value?: ReactNode; pct: number; tone?: "accent" | "cyan" | "violet"; className?: string }): ReactElement
export function Terminal(props: { title?: string; children: ReactNode; className?: string }): ReactElement
```

Referência visual: `docs/research/design-refresh/components/*.jsx` (recriações cosméticas — o port é TSX + Tailwind com tokens).

- [ ] **Step 1: Testes primeiro (`PageHeader.test.tsx` como modelo; os outros três seguem o mesmo padrão)**

```tsx
import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, it } from "vitest";
import { PageHeader } from "./PageHeader";

describe("PageHeader", () => {
  it("renders title as h1, subtitle and actions", () => {
    render(
      <MemoryRouter>
        <PageHeader title="Links" subtitle="128 links" actions={<button>Novo</button>} back={{ label: "Voltar", to: "/links" }} />
      </MemoryRouter>,
    );
    expect(screen.getByRole("heading", { level: 1, name: "Links" })).toBeInTheDocument();
    expect(screen.getByText("128 links")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Novo" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Voltar" })).toHaveAttribute("href", "/links");
  });
});
```

`StatCard.test`: value/label renderizam; com `accent` o value tem classe `text-brand-ink`, sem, `text-strong`.
`MeterBar.test`: `pct` fora de 0‑100 é clampado (style `transform: scaleX(0.5)` para `pct=50`; `scaleX(1)` para `pct=150`); `value` opcional renderiza.
`Terminal.test`: `children` em `<pre>`; título default `quark — zsh`; os 3 dots presentes (`data-testid="traffic-light"` ×3).

- [ ] **Step 2: Rodar (deve falhar)**

```bash
cd web && npx vitest run src/components/PageHeader.test.tsx src/components/StatCard.test.tsx src/components/MeterBar.test.tsx src/components/Terminal.test.tsx
```

Expected: FAIL (módulos não existem).

- [ ] **Step 3: Implementar**

`PageHeader.tsx`:

```tsx
import type { ReactNode } from "react";
import { Link } from "react-router-dom";

interface PageHeaderProps {
  title: ReactNode;
  subtitle?: ReactNode;
  actions?: ReactNode;
  /** Link de volta acima do título (padrão da tela de stats do mock). */
  back?: { label: string; to: string };
}

/** Cabeçalho de página do Quark DS: título display + subtítulo muted + ações à direita. */
export function PageHeader({ title, subtitle, actions, back }: PageHeaderProps) {
  return (
    <div className="mb-5">
      {back && (
        <Link to={back.to} className="mb-3 inline-block text-subtitle text-muted-foreground hover:text-foreground">
          {back.label}
        </Link>
      )}
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div className="min-w-0">
          <h1 className="font-heading text-page-title font-bold tracking-display text-strong">{title}</h1>
          {subtitle && <div className="mt-1 text-subtitle text-muted-foreground">{subtitle}</div>}
        </div>
        {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
      </div>
    </div>
  );
}
```

`StatCard.tsx`:

```tsx
import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

interface StatCardProps {
  value: ReactNode;
  label: ReactNode;
  /** Numeral em lime (métrica-herói); sem accent fica no strong. */
  accent?: boolean;
  className?: string;
}

/** KPI do Quark DS: numeral display grande + label muted, em card hairline. */
export function StatCard({ value, label, accent = false, className }: StatCardProps) {
  return (
    <div className={cn("rounded-lg border border-border bg-card p-[18px] shadow-card", className)}>
      <div className="text-[12.5px] text-muted-foreground">{label}</div>
      <div className={cn("mt-1.5 font-heading text-stat font-bold", accent ? "text-brand-ink" : "text-strong")}>{value}</div>
    </div>
  );
}
```

`MeterBar.tsx` (IMPORTANTE: animar `transform: scaleX`, nunca `width` — performance/hook de design):

```tsx
import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

const TONES = {
  accent: "bg-primary",
  cyan: "bg-chart-2",
  violet: "bg-chart-3",
} as const;

interface MeterBarProps {
  label: ReactNode;
  value?: ReactNode;
  /** 0–100 (clampado). */
  pct: number;
  tone?: keyof typeof TONES;
  className?: string;
}

/** Barra de distribuição do Quark DS (país/dispositivo/navegador). */
export function MeterBar({ label, value, pct, tone = "accent", className }: MeterBarProps) {
  const clamped = Math.max(0, Math.min(100, pct)) / 100;
  return (
    <div className={className}>
      <div className="mb-1.5 flex items-baseline justify-between gap-2 text-[13px]">
        <span className="text-foreground">{label}</span>
        {value != null && <span className="font-mono text-xs text-muted-foreground">{value}</span>}
      </div>
      <div className="h-[7px] overflow-hidden rounded-sm bg-secondary">
        <div
          className={cn("h-full origin-left rounded-sm transition-transform duration-500 ease-out", TONES[tone])}
          style={{ transform: `scaleX(${clamped})` }}
        />
      </div>
    </div>
  );
}
```

`Terminal.tsx`:

```tsx
import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

interface TerminalProps {
  title?: string;
  children: ReactNode;
  className?: string;
}

/** Janela de terminal do Quark DS (traffic lights + corpo mono). */
export function Terminal({ title = "quark — zsh", children, className }: TerminalProps) {
  return (
    <div className={cn("overflow-hidden rounded-lg border border-border bg-surface-input shadow-modal", className)}>
      <div className="flex items-center gap-2 border-b border-border bg-white/[0.02] px-4 py-3">
        {(["#ff5f57", "#febc2e", "#28c840"] as const).map((c) => (
          <span key={c} data-testid="traffic-light" className="size-[11px] rounded-full" style={{ background: c }} />
        ))}
        <span className="ml-2 font-mono text-xs text-muted-foreground">{title}</span>
      </div>
      <pre className="m-0 overflow-x-auto p-5 font-mono text-[13.5px] leading-[1.85] whitespace-pre-wrap text-foreground/85">{children}</pre>
    </div>
  );
}
```

- [ ] **Step 4: Rodar os testes (devem passar) + gate**

```bash
cd web && npx vitest run src/components/ && npm run lint && npm run typecheck
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add web/src/components && git commit -m "feat(web): componentes Quark DS v2 (PageHeader, StatCard, MeterBar, Terminal)"
```

---

### Task 7: Marca — assets oficiais no repo web

**Files:**
- Create: `web/src/assets/quark-lockup.svg` (copiar de `docs/research/design-refresh/assets/`)
- Verify: `web/public/favicon.svg` (já é o tile oficial), `web/src/components/brand/QuarkMark.tsx` (já é o glifo oficial)
- Delete: `web/src/assets/react.svg`, `web/src/assets/vite.svg` (não usados; conferir com grep antes)

- [ ] **Step 1: Conferir usos**

```bash
cd web && grep -rn "react.svg\|vite.svg\|hero.png" src/ index.html
```

Se `react.svg`/`vite.svg` não tiverem uso: deletar. `hero.png`: manter se usado (Login? conferir).

- [ ] **Step 2: Copiar o lockup**

```bash
cp ../docs/research/design-refresh/assets/quark-lockup.svg src/assets/quark-lockup.svg
```

- [ ] **Step 3: Gate + commit**

```bash
cd web && npm run typecheck && npm test && git add -A && git commit -m "chore(web): assets oficiais da marca (lockup) e limpeza de svgs de template"
```

---

### Task 8: Shell v2 — sidebar do mock + topbar com busca global e Novo link

**Files:**
- Modify: `web/src/app/Shell.tsx`, `web/src/app/Shell.test.tsx`
- Modify: `web/src/i18n/en.ts`, `web/src/i18n/pt-BR.ts` (keys novas)
- Reference: `docs/research/design-refresh/mock/sections/appShell.html`

**Interfaces:**
- Consumes: tokens (T4), Button/Input (T5).
- Produces: layout novo; o dialog global de criar link fica na Task 10 (aqui só o botão, navegando para `/links?new=1`).

Estrutura alvo (do mock, adaptada às features reais — manter TODA a lógica atual de RBAC/nav):

- **Sidebar** `w-16 sm:w-[250px] bg-sidebar border-r border-sidebar-border flex flex-col px-3 py-4`:
  1. Logo: `QuarkMark` 26px lime com `drop-shadow` glow + wordmark `font-heading font-bold text-lg tracking-tight` (como hoje).
  2. `WorkspaceSwitcher` MOVIDO da topbar para cá, logo abaixo do logo (card `border border-border rounded-[10px] p-2.5` — ajustar o componente SÓ se o container atual brigar com o novo lugar; a lógica não muda).
  3. Grupos de nav atuais (labels mono uppercase já existem): item `rounded-[9px] px-3 py-2 text-[14.5px] font-medium gap-3`, ativo = `bg-sidebar-accent text-sidebar-accent-foreground` (wash + lime — já mapeado nos tokens), inativo `text-sidebar-foreground/70 hover:bg-surface-hover`.
  4. `flex-1` spacer.
  5. Card do usuário no rodapé: avatar circular `size-[30px] rounded-full bg-primary text-primary-foreground font-heading font-bold text-xs` com iniciais do `me.data.email`, nome/email truncados, botão de logout (ícone `LogOut`) à direita — move o logout da topbar para cá (mesmo `handleLogout`).
  6. Linha "connected · host" atual permanece acima do card do usuário.
  7. Mobile (`< sm`): colapsa para ícones como hoje (labels/cards escondidos com `hidden sm:...`).
- **Topbar** `h-[62px] border-b border-border px-6 flex items-center gap-3 justify-between`:
  1. Busca global à esquerda: `div` com `bg-secondary border border-border rounded-[10px] max-w-[440px] flex-1` + ícone `Search` + `<input>` transparente; `Enter` → `navigate("/links?q=" + encodeURIComponent(term))`. Placeholder: `t("shell.searchPlaceholder")`.
  2. Direita: `LanguageSwitcher` (estilo mono compacto), toggle de tema (como hoje), botão primário `t("shell.newLink")` com ícone `Plus` → `navigate("/links?new=1")`.

- [ ] **Step 1: i18n keys**

Em `en.ts`, no bloco `shell`: `searchPlaceholder: "Search links…"`, `newLink: "New link"`. Em `pt-BR.ts`: `searchPlaceholder: "Buscar links…"`, `newLink: "Novo link"`.

- [ ] **Step 2: Atualizar `Shell.test.tsx` primeiro**

Ajustar/adicionar asserts: busca global presente (`getByPlaceholderText`), botão Novo link presente, logout AGORA no rodapé da sidebar (`getByRole("button", { name: /logout|sair/i })` continua achando), WorkspaceSwitcher continua renderizado. Rodar: deve FALHAR.

- [ ] **Step 3: Implementar o Shell conforme a estrutura alvo**

Preservar: `navGroups` (com RBAC), `handleLogout`, `apiHost`, tema. O `<main>` vira `p-6 sm:p-[26px_30px] overflow-auto` com `<Outlet />`.

- [ ] **Step 4: Rodar testes + dev visual**

```bash
cd web && npx vitest run src/app/Shell.test.tsx && npm run lint && npm run typecheck && npm test
```

Expected: PASS. `npm run dev` e comparar lado a lado com `mock/sections/appShell.html` aberto no browser (dark e light).

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(web): Shell v2 (sidebar mock, busca global, novo link, user card)"
```

---

### Task 9: Login v2

**Files:**
- Modify: `web/src/routes/Login.tsx`, `web/src/routes/Login.test.tsx` (se não existir teste, criar asserts básicos), i18n
- Reference: `docs/research/design-refresh/mock/sections/isLogin.html`

Adaptação (métodos REAIS mapeados no layout do mock — sem Google/senha, que não existem no produto):

- Fundo: página `bg-background` (ink) com duas camadas absolutas: glow radial lime (`bg-[radial-gradient(680px_460px_at_50%_-6%,rgba(198,249,78,.1),transparent_62%)]` — permitido: é o motif do DS, documentar com comentário) e dot-grid com máscara radial (copiar o pattern do mock, linhas 3‑4 do `isLogin.html`, para uma utility `login-backdrop` no `index.css` — MELHOR: definir as duas camadas como utilities `.bg-hero-glow` e `.bg-dot-grid` no index.css para reuso no AcceptInvite/Onboarding, Task 20/21).
- Card central `max-w-[400px] animate-rise`: glifo 42px com glow, `h1` "Entrar no quark" (`font-heading text-[26px] font-bold tracking-display text-strong`), sub muted.
- Dentro do card (`bg-card border border-input rounded-2xl p-6 shadow-modal`):
  1. Botão OIDC (quando `oidcEnabled`) — botão primário largura total (o método principal do produto).
  2. Estágio de e-mail discovery (`showEmailStage`) — input + botão outline, como hoje, com os estilos novos.
  3. Divider "ou" (hairlines) — como no mock.
  4. Seção do admin token (quando `adminLoginEnabled`): input mono + botão com ícone cadeado (`Lock`), estilo secundário mono do mock (borda hairline forte, `font-mono text-[13px]`).
- `LanguageSwitcher` no topo direito (mantém).
- TODA a lógica atual (estados, mutations, discovery, `?org=`) permanece intocada.

- [ ] **Step 1: i18n** — revisar bloco `login`: adicionar `title: "Sign in to quark"` / `"Entrar no quark"` (se não houver equivalente) e o que o layout pedir. NÃO remover keys usadas.
- [ ] **Step 2: Teste primeiro** — asserts: heading presente; com `oidc_enabled: true` o botão provider é o primeiro CTA; campo token presente quando `admin_login_enabled`. (Mock do fetch/api como os testes existentes do projeto fazem — ver `RequireAuth.test.tsx` como referência de mock de `api.me`.)
- [ ] **Step 3: Implementar** o layout acima (incl. utilities `.bg-hero-glow`/`.bg-dot-grid` no `index.css`).
- [ ] **Step 4: Gate** — `npx vitest run src/routes/Login.test.tsx && npm run lint && npm run typecheck && npm test` → PASS. Visual vs `isLogin.html`.
- [ ] **Step 5: Commit** — `git commit -m "feat(web): login v2 (glow, dot-grid, card central, metodos reais no layout do mock)"`

---

### Task 10: Links v2 — cards de link + chips + dialog global

**Files:**
- Modify: `web/src/routes/Links.tsx`, `web/src/components/LinkTable.tsx`, `web/src/components/LinkTable.test.tsx`, i18n
- Reference: `docs/research/design-refresh/mock/sections/isTabLinks.html`

O maior redesign estrutural: a lista de links deixa de parecer tabela e vira **stack de cards** (mock linhas 14‑43), MANTENDO TanStack Table por trás (headless: sorting/filtering/pagination continuam; muda só a renderização das rows).

- Header: `PageHeader` com `title` "Links" e `subtitle` "{n} links · {clicks} cliques" (dados que a tela já tem).
- Chips de filtro por TAG (o app tem tags; mock mostra pastas): pills horizontais com contagem, ativo = wash+lime. Usa o filtro de tags EXISTENTE se houver; se não houver filtro por tag hoje, os chips só refletem a busca `?q=` — NÃO criar API nova.
- Row → card: `card-hover flex items-center gap-4 rounded-lg border border-border bg-card shadow-card p-4`:
  - Ícone well (receita) com `Link2`.
  - Coluna principal: short `font-mono text-[14.5px] font-medium text-brand-ink` + badge `alias` quando alias (Badge secondary), dest truncado `text-[13px] text-muted-foreground max-w-[440px] truncate`, tags (chips coloridos atuais — `tag-color.ts` existente).
  - Clicks à direita: `font-heading font-bold text-lg text-strong` + label pequeno.
  - Ações: copy / stats / menu (dropdown existente com editar/QR/excluir) como botões `outline size-icon` 32px.
- Suportar os params novos: `?q=` preenche a busca; `?new=1` abre o `CreateLinkDialog` ao montar (e limpa o param).
- Empty state: manter o texto atual com a estética nova (well + muted).

- [ ] **Step 1: Testes primeiro** — atualizar `LinkTable.test.tsx`: rows agora são cards (`getAllByRole("listitem")` ou `data-testid="link-card"`), short/dest/clicks/ações continuam encontráveis; sorting/paginação seguem funcionando (interações existentes do teste).
- [ ] **Step 2: Implementar** a renderização em cards no `LinkTable.tsx` (manter colunas TanStack para sort; render custom por row). `Links.tsx`: PageHeader + chips + params `q`/`new`.
- [ ] **Step 3: Gate** — `npx vitest run src/components/LinkTable.test.tsx src/routes/ && npm run lint && npm run typecheck && npm test` → PASS. Visual vs `isTabLinks.html` (dark + light).
- [ ] **Step 4: Commit** — `git commit -m "feat(web): tela Links v2 (cards, chips, busca via ?q, dialog via ?new)"`

---

### Task 11: CreateLinkDialog + EditLinkDialog v2

**Files:**
- Modify: `web/src/components/CreateLinkDialog.tsx` (+ `.test.tsx`), `web/src/components/EditLinkDialog.tsx` (+ `.test.tsx`), i18n
- Reference: `docs/research/design-refresh/mock/sections/createOpen.html`

Aplicar o layout do mock aos campos REAIS (o dialog atual tem mais recursos que o mock — rules, variants, duration; eles permanecem, estilizados):

- Container: o Dialog v2 (T5) já dá scrim/blur/radius/rise.
- Ordem do mock: destino → grid 2col (alias com prefixo do domínio em mono + seletor/campo que o app já tenha) → tags (chips input) → TTL como CHIPS (`1h · 24h · 7d · 30d · ∞` — mapear para o `DurationField` existente: os chips setam valores; manter input custom para valores livres) → seção UTM em grid 3col → seções avançadas existentes (rules/variants) em `CollapsibleSection` com o estilo hairline → preview dashed (`border border-dashed border-input rounded-[10px] bg-secondary p-3.5` com o short mono lime) → footer cancel/submit.
- Nenhum campo removido; nenhuma validação alterada.

- [ ] **Step 1: Testes** — os testes atuais dos dialogs devem continuar passando (fluxos de submit/validação); adicionar assert do preview quando alias digitado. Rodar antes (baseline verde), depois da mudança (verde de novo).
- [ ] **Step 2: Implementar.**
- [ ] **Step 3: Gate** + visual vs `createOpen.html`.
- [ ] **Step 4: Commit** — `git commit -m "feat(web): dialogs de link v2 (layout mock, ttl chips, preview)"`

---

### Task 12: Analytics + LinkStats v2 (StatsView/StatsCharts)

**Files:**
- Modify: `web/src/components/StatsView.tsx` (+ test), `web/src/components/StatsCharts.tsx` (+ test), `web/src/components/RecentEventsTable.tsx` (+ test), `web/src/routes/Analytics.tsx` (+ test), `web/src/routes/LinkStats.tsx`
- Reference: `docs/research/design-refresh/mock/sections/isTabAnalytics.html`

O mock de analytics é a visão POR LINK (short mono no h1 + back) = `LinkStats`; a mesma linguagem vale para o agregado (`Analytics`).

- KPI row: 4 `StatCard` (`grid grid-cols-2 lg:grid-cols-4 gap-3.5`): total (accent), únicos, top país, top device (os dois últimos `text-[22px]` — usar StatCard com value menor via children).
- Chart de barras/dia (recharts existente): barras com gradiente lime (`<linearGradient>` de `--chart-1` para `rgba(198,249,78,.35)`), radius topo 4, grid hairline, tooltip com `bg-popover border-border`.
- Distribuições país/device/browser: substituir a renderização atual por `MeterBar` (país = `tone="cyan"`, device = `violet`, browser = `accent`), com `%` mono à direita — em 2 cards lado a lado como no mock.
- Recent events (`RecentEventsTable`): grid com header uppercase mono 11px + rows hairline, contagem de bots mono no canto (dado existente; se não houver, omitir).
- `LinkStats`: `PageHeader` com `back` para `/links`, título = short mono, subtitle = dest.

- [ ] **Step 1: Testes** — atualizar os 4 testes afetados (KPIs viram StatCard: buscar por label/valor; MeterBar presente por `getAllByText` de %). Baseline → red → green.
- [ ] **Step 2: Implementar.**
- [ ] **Step 3: Gate** + visual vs `isTabAnalytics.html`.
- [ ] **Step 4: Commit** — `git commit -m "feat(web): analytics/stats v2 (StatCards, MeterBars, chart lime, eventos)"`

---

### Task 13: Domains v2

**Files:** `web/src/routes/Domains.tsx` (+ test), i18n. **Reference:** `sections/isTabDomains.html`.

- `PageHeader` título/sub + botão primário "Adicionar domínio" (fluxo existente).
- Cards `card-hover`: well com `Globe` (well NEUTRO `bg-secondary`, não wash — mock linha 10), host `font-mono text-[15px] text-strong`, badge `primário/default` quando principal (wash+lime), sub "{n} links", status dot + label à direita (verificado = `bg-primary`, pendente = amber → usar `bg-chart-4`? NÃO: pendente = `bg-[--color-chart-5]`… usar semântica: verificado `bg-primary`, pendente `bg-muted-foreground`, erro `bg-destructive`).
- Fluxos de verificação/DNS existentes mantidos dentro dos cards/collapsibles com o estilo novo.

- [ ] Steps: testes baseline → implementar → gate → `git commit -m "feat(web): dominios v2"`

---

### Task 14: Members v2 (+ AcceptInvite na Task 21)

**Files:** `web/src/routes/Members.tsx` (+ test), i18n. **Reference:** `sections/isTabMembers.html`.

- `PageHeader` + botão primário "Convidar" (dialog/fluxo existente).
- Lista em UM card (`rounded-lg border border-border bg-card overflow-hidden`): rows `flex items-center gap-3.5 px-4 py-4 border-b border-border last:border-b-0`, avatar `size-9 rounded-full font-heading font-bold text-[13px] text-primary-foreground` com iniciais e cor derivada do email (reusar `tag-color.ts` se aplicável), nome strong + subline muted, role em pill outline à direita (+ ações de admin existentes: dropdown/remover).
- Seção de convites pendentes existente: mesma linguagem (card separado, rows hairline).

- [ ] Steps: testes → implementar → gate → `git commit -m "feat(web): members v2"`

---

### Task 15: Tokens v2

**Files:** `web/src/routes/Tokens.tsx` (+ test), `web/src/components/CreateTokenDialog.tsx`, i18n. **Reference:** `sections/isTabTokens.html`.

- `PageHeader` + "Criar token" primário.
- Cards `card-hover`: nome strong; prefixo mascarado `font-mono text-[12.5px] text-muted-foreground` (`qk_live_a1b2••••••••`); revoke = botão outline com texto `text-destructive` (mock linha 12); linha de meta: scopes em chips mono (`bg-secondary rounded-md px-2 py-0.5 font-mono text-[11.5px]`) + `· rate` + `· last used` muted.
- `CreateTokenDialog`: estilo v2 (o Dialog da T5 já cobre; revisar espaçamentos/labels).

- [ ] Steps: testes → implementar → gate → `git commit -m "feat(web): tokens v2"`

---

### Task 16: Webhooks v2

**Files:** `web/src/routes/Webhooks.tsx` (+ test), i18n. **Reference:** `sections/isTabWebhooks.html`.

- `PageHeader` + "Adicionar webhook" primário.
- Cards `card-hover`: status dot (ativo `bg-primary`, falhando `bg-destructive`, pausado `bg-muted-foreground`) + URL mono truncada strong; badge do tipo + botão "Testar" (fluxo existente) à direita; linha meta: eventos em chip mono `bg-secondary` + `· {n} entregues` muted.
- Painéis de entregas/segredo existentes: mesma linguagem (tabela hairline v2).

- [ ] Steps: testes → implementar → gate → `git commit -m "feat(web): webhooks v2"`

---

### Task 17: Extensions + ExtensionDetail v2 (extrapolada)

**Files:** `web/src/routes/Extensions.tsx` (+ test), `web/src/routes/ExtensionDetail.tsx` (+ test), i18n.

Sem mock — compor com o DS:

- Extensions: `PageHeader` + grid de cards `card-hover` (2‑3 col): well com o ícone da integração, nome strong, descrição muted 13px, badge de status (conectado = default/wash; disponível = secondary).
- ExtensionDetail (648 linhas — só restyle, zero mudança de fluxo): `PageHeader` com `back` para `/extensions`; seções em Cards v2; snippets/exemplos de configuração migram para `Terminal`; formulários com inputs v2.

- [ ] Steps: testes → implementar → gate → `git commit -m "feat(web): central de integracoes v2"`

---

### Task 18: Import v2 (extrapolada)

**Files:** `web/src/routes/Import.tsx` (+ test), i18n.

- `PageHeader`; área de upload como well tracejado grande (`border border-dashed border-input rounded-lg bg-surface-input p-10 text-center`) com ícone `Upload`; instruções/formato em `Terminal` (ex. cabeçalho CSV esperado — usar os textos i18n existentes); resultados/erros de import na tabela hairline v2.

- [ ] Steps: testes → implementar → gate → `git commit -m "feat(web): import v2"`

---

### Task 19: Pixels v2 (extrapolada)

**Files:** `web/src/routes/Pixels.tsx` (+ test), i18n.

- `PageHeader` + botão primário criar; lista em cards `card-hover` (nome strong, plataforma como badge, id mono muted, ações outline) seguindo o padrão Tokens/Webhooks.

- [ ] Steps: testes → implementar → gate → `git commit -m "feat(web): pixels v2"`

---

### Task 20: SsoDomains + OidcProvider v2 (extrapoladas)

**Files:** `web/src/routes/SsoDomains.tsx` (+ test), `web/src/routes/OidcProvider.tsx` (+ test), i18n.

- Ambas: `PageHeader`; formulários dentro de Card v2 com labels 13px muted e inputs well; listas (domínios SSO) como rows hairline com status dot + host mono; instruções/URLs de callback em bloco mono `bg-secondary rounded-[10px] p-3 font-mono text-[12.5px]` (ou `Terminal` quando for multi-linha).

- [ ] Steps: testes → implementar → gate → `git commit -m "feat(web): sso domains + oidc provider v2"`

---

### Task 21: Onboarding + AcceptInvite v2 (extrapoladas)

**Files:** `web/src/routes/Onboarding.tsx` (+ test), `web/src/routes/AcceptInvite.tsx` (+ test), `web/src/components/CreateWorkspaceForm.tsx` (+ test), i18n.

- Ambas são páginas "fora do Shell" → usar o backdrop do Login (`bg-hero-glow` + `bg-dot-grid` da Task 9) com card central `animate-rise` (mesma moldura do login: glifo + título display + card hairline).
- AcceptInvite: estados aceitar/erro/expirado com a estética v2 (well de ícone + muted).
- CreateWorkspaceForm: inputs v2, botão primário.

- [ ] Steps: testes → implementar → gate → `git commit -m "feat(web): onboarding + accept invite v2"`

---

### Task 22: QA final — varredura visual, e2e e gate completo

**Files:** nenhum novo (correções pontuais que a varredura achar).

- [ ] **Step 1: Gate completo**

```bash
cd web && npm run lint && npm run typecheck && npm test && npm run build
```

Expected: PASS.

- [ ] **Step 2: Varredura visual dark + light**

`npm run dev` e percorrer TODAS as 14 telas + login nos dois temas, lado a lado com o mock (`mock/Quark.dc.html` aberto no browser — abrir direto do disco; ele carrega `support.js` da mesma pasta) e com `screenshots/` do projeto de design quando preciso. Checklist por tela: superfícies/hairlines correspondem; lime só em ação primária/ativo/métrica; mono nos códigos/eyebrows; radii 12/16; hover lift nos cards interativos; `animate-rise` na entrada; nenhum hex hardcoded fora das exceções.

- [ ] **Step 3: E2e**

```bash
cd web && npm run e2e
```

Expected: PASS (se o ambiente docker do IdP não estiver disponível, rodar o subconjunto que não depende dele e registrar quais specs ficaram pendentes).

- [ ] **Step 4: Limpeza**

`grep -rn "drop-shadow-\[0_0_" src/` e hexes fora de `index.css`/marca → migrar para tokens/utilities. Remover CSS morto do tema v1 se sobrar.

- [ ] **Step 5: Commit final**

```bash
git add -A && git commit -m "polish(web): varredura visual final do design refresh"
```

---

## Fora do plano (pós-merge, sessão principal)

Review final da branch (superpowers:requesting-code-review), merge na main (dispara auto-deploy), atualização da issue no Linear e verificação do deploy — seguem o fluxo padrão do projeto, não são tasks deste plano.

## Self-review do plano (feito)

- Cobertura da spec: fundação (T1‑T7), telas fiéis (T8‑T16), extrapoladas (T17‑T21), engenharia (T1‑T3), verificação (T22). Landing: fora (consistente com a spec).
- Sem placeholders: cada task tem arquivos exatos, referência de mock e receita/código.
- Consistência de tipos: assinaturas de PageHeader/StatCard/MeterBar/Terminal definidas na T6 e consumidas nas T8+ com os mesmos nomes.
