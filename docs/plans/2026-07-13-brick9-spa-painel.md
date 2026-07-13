# Tijolo 9 — SPA do painel (plano de implementação)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Um painel web (SPA React) em `web/` que consome a API `/admin/*` do quark para o operador único gerenciar links, ver analytics e a blocklist — com UI/UX seguindo as heurísticas de Nielsen.

**Architecture:** App React+Vite+TS em `web/` (monorepo), build estático deployado à parte; o binário Rust segue API-only. Camada de dados isolada (cliente HTTP tipado + TanStack Query); telas montadas com shadcn/ui (Radix, acessível). Auth por `x-admin-token` em `localStorage`.

**Tech Stack:** React 18, Vite, TypeScript, Tailwind + shadcn/ui (Radix), TanStack Query + TanStack Table, React Router, Recharts, Vitest + Testing Library.

## Método (leia antes de executar)

- **Tarefas de LÓGICA/config (1, 2, 8):** código completo no plano.
- **Tarefas de TELA/UI (3–7):** o plano dá o **contrato** (dados, props, estados, endpoints, componentes shadcn, checklist de Nielsen) e os **testes de aceitação (Vitest/Testing Library)** completos. A **implementação visual é produzida via a skill `superpowers:frontend-design`** durante a execução — o implementer invoca essa skill para construir o componente satisfazendo o contrato e fazendo os testes passarem. Isto é intencional (requisito do usuário) e substitui "TSX verbatim no plano".
- **Pré-requisito:** Node ≥ 20 + npm (confirmado no ambiente: v24 / 11). Todos os comandos rodam a partir de `web/`.

## Global Constraints

- SPA em `web/`; **deploy separado** (build estático `web/dist`); o binário Rust **não** serve o SPA.
- Auth por header `x-admin-token` = `QUARK_ADMIN_TOKEN`; token em `localStorage`; **sem cookie/sessão**.
- Base da API por `VITE_API_BASE_URL` (build-time; vazio = mesma origem).
- `401` de qualquer chamada → limpa token e volta pro login.
- **UI/UX seguindo Nielsen é requisito**: estados **loading / vazio / erro** desenhados em toda tela; confirmação antes de deletar; acessibilidade (Radix + contraste AA nos temas claro/escuro).
- **Toda tarefa de UI usa a skill `frontend-design`.**
- SPA OSS é **AGPL** (mesmo repo).

## File Structure

- `web/package.json`, `web/vite.config.ts`, `web/tsconfig*.json`, `web/tailwind.config.js`, `web/index.html`, `web/.eslintrc` — tooling.
- `web/src/lib/types.ts` — tipos da API.
- `web/src/lib/api.ts` — cliente HTTP tipado (x-admin-token, base, 401).
- `web/src/lib/auth.ts` — token em localStorage.
- `web/src/lib/queries.ts` — hooks TanStack Query por recurso.
- `web/src/app/` — shell (layout, sidebar, tema, toaster), router, guarda de rota.
- `web/src/routes/` — `Login.tsx`, `Links.tsx`, `LinkStats.tsx`, `Blocklist.tsx`.
- `web/src/components/` — componentes shadcn/ui + compostos (LinkTable, CreateLinkDialog, etc.).
- `web/src/**/*.test.tsx` — testes Vitest.
- `.gitignore` (raiz) — `node_modules/`, `web/dist`.
- `.github/workflows/ci.yml` — job `web`.

---

## Task 1: Scaffold `web/` + tooling

**Files:** Create `web/*` (scaffold), Modify `.gitignore`.

- [ ] **Step 1: Criar o app Vite React-TS**

```bash
cd C:/Users/L-SALDANHA/pessoal/quark
npm create vite@latest web -- --template react-ts
cd web && npm install
```

- [ ] **Step 2: Tailwind + shadcn/ui + libs**

```bash
cd web
npm install -D tailwindcss postcss autoprefixer @types/node vitest @testing-library/react @testing-library/jest-dom @testing-library/user-event jsdom
npm install @tanstack/react-query @tanstack/react-table react-router-dom recharts
npx tailwindcss init -p
```

Configurar Tailwind (`web/tailwind.config.js` → `content: ["./index.html","./src/**/*.{ts,tsx}"]`, com `darkMode: "class"`), o `@tailwind base/components/utilities` no `src/index.css`, e o alias `@` → `src` no `vite.config.ts` e `tsconfig.json` (`baseUrl:"."`, `paths: {"@/*":["src/*"]}`). Inicializar shadcn:

```bash
npx shadcn@latest init -d
npx shadcn@latest add button input table dialog dropdown-menu sonner card badge skeleton alert-dialog tabs
```

- [ ] **Step 3: Configurar Vitest**

Em `web/vite.config.ts`, adicionar o bloco `test`:

```ts
/// <reference types="vitest" />
// ...defineConfig({ ...plugins, resolve alias..., test: {
//   globals: true, environment: "jsdom", setupFiles: "./src/test-setup.ts",
// }})
```

Criar `web/src/test-setup.ts`:
```ts
import "@testing-library/jest-dom/vitest";
```

Adicionar scripts em `web/package.json`:
```json
"scripts": {
  "dev": "vite",
  "build": "tsc -b && vite build",
  "test": "vitest run",
  "lint": "eslint . --max-warnings 0",
  "typecheck": "tsc --noEmit"
}
```

- [ ] **Step 4: Smoke test**

Criar `web/src/smoke.test.ts`:
```ts
import { describe, it, expect } from "vitest";
describe("smoke", () => {
  it("soma", () => { expect(1 + 1).toBe(2); });
});
```

- [ ] **Step 5: Ignorar artefatos de node no git**

Adicionar ao `.gitignore` da raiz do repo:
```
# frontend
node_modules/
web/dist/
web/node_modules/
```

- [ ] **Step 6: Verificar build + test + commit**

Run (em `web/`): `npm run build && npm run test && npm run lint`
Expected: build gera `web/dist`; smoke test passa; lint limpo.

```bash
git add web .gitignore
git commit -m "chore(web): scaffold Vite+React+TS+Tailwind+shadcn/ui+Vitest"
```

---

## Task 2: Tipos + cliente HTTP + auth (lógica)

**Files:** Create `web/src/lib/types.ts`, `web/src/lib/api.ts`, `web/src/lib/auth.ts`, `web/src/lib/api.test.ts`, `web/src/lib/auth.test.ts`.

**Interfaces (Produces):** funções tipadas usadas por todas as telas — assinaturas abaixo, verbatim.

- [ ] **Step 1: Tipos da API**

`web/src/lib/types.ts`:
```ts
export interface Link {
  id: number;
  code: string;
  alias?: string;
  url: string;
  expiry: number | null;
  created: number;
}
export interface ListLinksResponse { links: Link[]; next_after: number | null; }
export interface CreateLinkRequest { url: string; alias?: string; ttl?: number; }
export interface CreateLinkResponse { code: string; url: string; }
export interface ClickEvent {
  id: number; ts: number;
  referer?: string | null; country?: string | null; user_agent?: string | null;
}
export interface Aggregates {
  total: number; first_ts: number; last_ts: number;
  per_day: Record<string, number>;
  per_country: Record<string, number>;
  per_device: Record<string, number>;
}
export interface Stats { aggregates: Aggregates; recent: ClickEvent[]; }
export interface BlocklistResponse { domains: string[]; }
export interface PatchLinkRequest { url?: string; ttl?: number | null; }
```

- [ ] **Step 2: auth (token em localStorage)**

`web/src/lib/auth.ts`:
```ts
const KEY = "quark_admin_token";
export function getToken(): string | null { return localStorage.getItem(KEY); }
export function setToken(t: string): void { localStorage.setItem(KEY, t); }
export function clearToken(): void { localStorage.removeItem(KEY); }
export function hasToken(): boolean { return getToken() !== null; }
```

`web/src/lib/auth.test.ts`:
```ts
import { describe, it, expect, beforeEach } from "vitest";
import { getToken, setToken, clearToken, hasToken } from "./auth";

describe("auth token store", () => {
  beforeEach(() => localStorage.clear());
  it("set/get/has/clear", () => {
    expect(hasToken()).toBe(false);
    setToken("segredo");
    expect(getToken()).toBe("segredo");
    expect(hasToken()).toBe(true);
    clearToken();
    expect(getToken()).toBeNull();
  });
});
```

- [ ] **Step 3: cliente HTTP tipado**

`web/src/lib/api.ts`:
```ts
import { getToken } from "./auth";
import type {
  ListLinksResponse, CreateLinkRequest, CreateLinkResponse,
  Stats, BlocklistResponse, PatchLinkRequest,
} from "./types";

const BASE: string = (import.meta.env.VITE_API_BASE_URL as string | undefined) ?? "";

let onUnauthorized: () => void = () => {};
export function setUnauthorizedHandler(fn: () => void): void { onUnauthorized = fn; }

export class ApiError extends Error {
  constructor(public status: number, message: string) { super(message); }
}

async function req(path: string, opts: RequestInit = {}): Promise<Response> {
  const headers = new Headers(opts.headers);
  const token = getToken();
  if (token) headers.set("x-admin-token", token);
  if (opts.body) headers.set("content-type", "application/json");
  const res = await fetch(BASE + path, { ...opts, headers });
  if (res.status === 401) { onUnauthorized(); throw new ApiError(401, "não autorizado"); }
  return res;
}

async function jsonOrThrow<T>(res: Response): Promise<T> {
  if (!res.ok) throw new ApiError(res.status, await res.text().catch(() => res.statusText));
  return (await res.json()) as T;
}

export const api = {
  async createLink(body: CreateLinkRequest): Promise<CreateLinkResponse> {
    return jsonOrThrow(await req("/", { method: "POST", body: JSON.stringify(body) }));
  },
  async listLinks(params: { after?: number; limit?: number } = {}): Promise<ListLinksResponse> {
    const q = new URLSearchParams();
    if (params.after != null) q.set("after", String(params.after));
    if (params.limit != null) q.set("limit", String(params.limit));
    const qs = q.toString();
    return jsonOrThrow(await req(`/admin/links${qs ? `?${qs}` : ""}`));
  },
  async deleteLink(code: string): Promise<void> {
    const res = await req(`/admin/links/${encodeURIComponent(code)}`, { method: "DELETE" });
    if (!res.ok) throw new ApiError(res.status, await res.text().catch(() => res.statusText));
  },
  async patchLink(code: string, body: PatchLinkRequest): Promise<void> {
    const res = await req(`/admin/links/${encodeURIComponent(code)}`, {
      method: "PATCH", body: JSON.stringify(body),
    });
    if (!res.ok) throw new ApiError(res.status, await res.text().catch(() => res.statusText));
  },
  async getStats(code: string): Promise<Stats> {
    return jsonOrThrow(await req(`/${encodeURIComponent(code)}/stats`));
  },
  async listBlocked(): Promise<BlocklistResponse> {
    return jsonOrThrow(await req("/admin/blocklist"));
  },
  async addBlocked(domain: string): Promise<void> {
    const res = await req("/admin/blocklist", { method: "POST", body: JSON.stringify({ domain }) });
    if (!res.ok) throw new ApiError(res.status, await res.text().catch(() => res.statusText));
  },
  async removeBlocked(domain: string): Promise<void> {
    const res = await req("/admin/blocklist", { method: "DELETE", body: JSON.stringify({ domain }) });
    if (!res.ok) throw new ApiError(res.status, await res.text().catch(() => res.statusText));
  },
};
```

- [ ] **Step 4: testes do cliente**

`web/src/lib/api.test.ts`:
```ts
import { describe, it, expect, beforeEach, vi } from "vitest";
import { api, ApiError, setUnauthorizedHandler } from "./api";
import { setToken } from "./auth";

describe("api client", () => {
  beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });

  it("envia x-admin-token e parseia JSON", async () => {
    setToken("segredo");
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ links: [], next_after: null }), { status: 200 }),
    );
    const r = await api.listLinks({ limit: 10 });
    expect(r.links).toEqual([]);
    const [, init] = fetchMock.mock.calls[0];
    expect(new Headers(init!.headers).get("x-admin-token")).toBe("segredo");
  });

  it("401 dispara onUnauthorized e lança ApiError", async () => {
    const onUnauth = vi.fn();
    setUnauthorizedHandler(onUnauth);
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 401 }));
    await expect(api.listLinks()).rejects.toBeInstanceOf(ApiError);
    expect(onUnauth).toHaveBeenCalledOnce();
  });

  it("erro !ok vira ApiError com status", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("destino bloqueado", { status: 403 }));
    await expect(api.createLink({ url: "https://x.com" })).rejects.toMatchObject({ status: 403 });
  });
});
```

- [ ] **Step 5: rodar + commit**

Run (em `web/`): `npm run test && npm run typecheck`
Expected: testes de api/auth passam; typecheck limpo.

```bash
git add web/src/lib
git commit -m "feat(web): tipos + cliente HTTP (x-admin-token, 401→logout) + auth store, com testes"
```

---

## Task 3: Shell + router + tema + Login  ·  **usa a skill frontend-design**

**Files:** Create `web/src/app/{App.tsx,router.tsx,Shell.tsx,theme.tsx,RequireAuth.tsx}`, `web/src/lib/queries.ts` (QueryClient + provider), `web/src/routes/Login.tsx`, `web/src/routes/Login.test.tsx`; Modify `web/src/main.tsx`.

**Interfaces (Consumes):** `api`, `setUnauthorizedHandler`, `auth` (Task 2).

**Contrato:**
- `QueryClientProvider` no topo; `setUnauthorizedHandler` conectado a `clearToken()` + navegação pro `/login`.
- **Rotas** (React Router): `/login` (público); `/links`, `/links/:code`, `/blocklist` (protegidas por `RequireAuth` — sem token → redireciona `/login`).
- **Shell**: sidebar (Links · Blocklist), header com **toggle de tema** (claro/escuro via classe no `<html>`, persistido em localStorage) e botão **Sair** (limpa token → `/login`). `<Toaster>` (sonner) montado.
- **Login**: input de token + botão Entrar. Ao submeter: `setToken` → sonda `api.listLinks({limit:1})`; sucesso → `/links`; `401`/erro → mostra "token inválido" e limpa o token.

**Checklist Nielsen desta tela:** loading no botão durante a sonda; erro visível e claro; foco no input ao abrir; enter submete; contraste AA nos dois temas.

- [ ] **Step 1: teste de aceitação (falha primeiro)**

`web/src/routes/Login.test.tsx`:
```tsx
import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Login } from "./Login";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}><MemoryRouter>{ui}</MemoryRouter></QueryClientProvider>;
}

describe("Login", () => {
  beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });

  it("token válido guarda e a sonda é chamada", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ links: [], next_after: null }), { status: 200 }),
    );
    render(wrap(<Login />));
    await userEvent.type(screen.getByLabelText(/token/i), "segredo");
    await userEvent.click(screen.getByRole("button", { name: /entrar/i }));
    expect(localStorage.getItem("quark_admin_token")).toBe("segredo");
  });

  it("token inválido mostra erro", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 401 }));
    render(wrap(<Login />));
    await userEvent.type(screen.getByLabelText(/token/i), "errado");
    await userEvent.click(screen.getByRole("button", { name: /entrar/i }));
    expect(await screen.findByText(/token inválido/i)).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: implementar com a skill frontend-design**

Invoque **`superpowers:frontend-design`** para construir o shell, o router, o provider de tema e a tela de Login satisfazendo o Contrato acima e fazendo `Login.test.tsx` passar. Use os componentes shadcn (`input`, `button`, `card`, `sonner`). Mantenha o `<label htmlFor>` associado ao input (o teste usa `getByLabelText(/token/i)`) e o botão com nome acessível "Entrar".

- [ ] **Step 3: rodar + commit**

Run (em `web/`): `npm run test && npm run typecheck && npm run lint`
Expected: `Login.test.tsx` verde; typecheck/lint limpos.

```bash
git add web/src
git commit -m "feat(web): shell + router + tema claro/escuro + Login (token→sonda→logout no 401)"
```

---

## Task 4: Tela de Links — lista, paginação, busca, copiar  ·  **usa frontend-design**

**Files:** Create `web/src/routes/Links.tsx`, `web/src/components/LinkTable.tsx`, `web/src/routes/Links.test.tsx`; Modify `web/src/lib/queries.ts` (hook `useLinks`).

**Interfaces (Consumes):** `api.listLinks` (Task 2); `Link` (types).

**Contrato:**
- `useLinks()` (TanStack Query, `useInfiniteQuery`): página inicial `listLinks({limit:50})`; `getNextPageParam` = `next_after`; botão **"Carregar mais"** quando houver.
- **Tabela** (TanStack Table): colunas code / url (truncada + `title`) / alias / criado (data legível) / expira. Cada linha: botão **Copiar** (copia `${window.location.origin? ou base pública}/${code}` via `navigator.clipboard`, com toast "copiado!"), e menu de ações (Editar, Deletar — ligados na Task 5). Uma coluna de ações.
- **Busca** (input no topo): filtra client-side por substring em `code`/`url`/`alias` sobre o que já foi carregado (documentar: só filtra o carregado).
- **Estados:** loading = skeleton de linhas; vazio = card "nenhum link ainda" + CTA Criar (dialog da Task 5); erro = alerta + "tentar de novo".

**Checklist Nielsen:** skeleton no loading; vazio desenhado; erro recuperável; feedback de "copiado"; cabeçalhos e ações consistentes; navegação por teclado (Radix).

- [ ] **Step 1: teste de aceitação (falha primeiro)**

`web/src/routes/Links.test.tsx`:
```tsx
import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Links } from "./Links";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}><MemoryRouter>{ui}</MemoryRouter></QueryClientProvider>;
}

describe("Links", () => {
  beforeEach(() => { localStorage.setItem("quark_admin_token", "s"); vi.restoreAllMocks(); });

  it("renderiza os links carregados", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify({
      links: [{ id: 1, code: "6lB362J", url: "https://exemplo.com/a", alias: "promo", expiry: null, created: 1700000000 }],
      next_after: null,
    }), { status: 200 }));
    render(wrap(<Links />));
    expect(await screen.findByText("6lB362J")).toBeInTheDocument();
    expect(screen.getByText(/exemplo\.com\/a/)).toBeInTheDocument();
  });

  it("busca filtra a lista carregada", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify({
      links: [
        { id: 1, code: "AAA0000", url: "https://gato.com", expiry: null, created: 1 },
        { id: 2, code: "BBB1111", url: "https://cachorro.com", expiry: null, created: 2 },
      ],
      next_after: null,
    }), { status: 200 }));
    render(wrap(<Links />));
    await screen.findByText("AAA0000");
    await userEvent.type(screen.getByRole("searchbox"), "cachorro");
    expect(screen.queryByText("AAA0000")).not.toBeInTheDocument();
    expect(screen.getByText("BBB1111")).toBeInTheDocument();
  });

  it("estado vazio", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify({ links: [], next_after: null }), { status: 200 }));
    render(wrap(<Links />));
    expect(await screen.findByText(/nenhum link ainda/i)).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: implementar com frontend-design**

Invoque **`superpowers:frontend-design`** para construir `Links.tsx` + `LinkTable.tsx` + o hook `useLinks` satisfazendo o Contrato e fazendo `Links.test.tsx` passar. O input de busca deve ter `role="searchbox"` (use `<Input type="search">`).

- [ ] **Step 3: rodar + commit**

Run (em `web/`): `npm run test && npm run typecheck && npm run lint`

```bash
git add web/src
git commit -m "feat(web): tela de Links — lista paginada (keyset), busca client-side, copiar URL, estados"
```

---

## Task 5: Links — criar / editar / deletar  ·  **usa frontend-design**

**Files:** Create `web/src/components/{CreateLinkDialog.tsx,EditLinkDialog.tsx}`, `web/src/components/CreateLinkDialog.test.tsx`; Modify `web/src/lib/queries.ts` (mutations), `web/src/routes/Links.tsx` (ligar ações).

**Interfaces (Consumes):** `api.createLink/patchLink/deleteLink`; validação espelha a API.

**Contrato:**
- **Criar** (dialog): campos URL (obrigatória) + alias (opcional) + TTL segundos (opcional). Validação client-side **antes** do submit: URL começa com `http://`/`https://`; alias, se preenchido, **não** pode ser 7 chars base62 no domínio (espelha a rejeição da API) — mostrar erro inline. Sucesso → toast + fecha + invalida `useLinks`. Erros mapeados: 409 "alias em uso", 403 "destino não permitido/bloqueado", 429 "muitas requisições".
- **Editar** (dialog): URL e/ou TTL; mesma validação de URL; `patchLink`; sucesso → toast + invalida.
- **Deletar**: `AlertDialog` de **confirmação** ("Isto não pode ser desfeito"); `deleteLink`; sucesso → toast + invalida.

**Checklist Nielsen:** confirmação destrutiva; validação preventiva com mensagens claras; loading nos botões; cancelar em todo dialog; foco preso no dialog (Radix).

- [ ] **Step 1: teste de aceitação (falha primeiro)**

`web/src/components/CreateLinkDialog.test.tsx`:
```tsx
import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { CreateLinkDialog } from "./CreateLinkDialog";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
  return <QueryClientProvider client={qc}>{ui}</QueryClientProvider>;
}

describe("CreateLinkDialog", () => {
  beforeEach(() => { localStorage.setItem("quark_admin_token", "s"); vi.restoreAllMocks(); });

  it("rejeita URL não http(s) sem chamar a API", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch");
    render(wrap(<CreateLinkDialog open onOpenChange={() => {}} />));
    await userEvent.type(screen.getByLabelText(/url/i), "ftp://nope");
    await userEvent.click(screen.getByRole("button", { name: /criar/i }));
    expect(await screen.findByText(/url inválida|http/i)).toBeInTheDocument();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("URL válida chama a API", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ code: "6lB362J", url: "https://ok.com" }), { status: 200 }),
    );
    render(wrap(<CreateLinkDialog open onOpenChange={() => {}} />));
    await userEvent.type(screen.getByLabelText(/url/i), "https://ok.com");
    await userEvent.click(screen.getByRole("button", { name: /criar/i }));
    expect(fetchMock).toHaveBeenCalledOnce();
  });
});
```

- [ ] **Step 2: implementar com frontend-design**

Invoque **`superpowers:frontend-design`** para construir os dialogs de criar/editar, o `AlertDialog` de deletar e as mutations, satisfazendo o Contrato e fazendo o teste passar. Ligue as ações na `Links.tsx` (Task 4). A validação de alias-base62 reusa a mesma regra da API (7 chars, alfabeto base62, ≤ 2^40-1) — implemente um helper `isNumericCode(s)` em `web/src/lib/codeguard.ts` e teste-o.

- [ ] **Step 3: rodar + commit**

Run (em `web/`): `npm run test && npm run typecheck && npm run lint`

```bash
git add web/src
git commit -m "feat(web): criar/editar/deletar links (validação client-side + confirmação + invalidação)"
```

---

## Task 6: Tela de Stats por link  ·  **usa frontend-design**

**Files:** Create `web/src/routes/LinkStats.tsx`, `web/src/components/{StatsCharts.tsx,RecentEventsTable.tsx}`, `web/src/routes/LinkStats.test.tsx`; Modify `web/src/lib/queries.ts` (`useStats`).

**Interfaces (Consumes):** `api.getStats` → `Stats { aggregates, recent }`.

**Contrato:**
- Rota `/links/:code`. `useStats(code)`.
- **Cartões:** total de cliques, primeiro clique (data de `first_ts`), último clique (`last_ts`).
- **Gráficos (Recharts):** `per_day` = série temporal (linha ou barra, ordenado por data); `per_country` = barras top-N (ordenado desc); `per_device` = rosca/pizza.
- **Tabela de eventos recentes:** `recent` (ts legível, país, device derivado? não — a API manda `user_agent`; mostrar país + referer + horário). Mais novo primeiro.
- **Estado vazio:** `total === 0` → "sem cliques ainda"; loading = skeleton; erro = alerta + voltar.

**Checklist Nielsen:** vazio/loading/erro; gráficos com rótulos/legenda; tabela acessível; botão voltar pra Links.

- [ ] **Step 1: teste de aceitação (falha primeiro)**

`web/src/routes/LinkStats.test.tsx`:
```tsx
import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { LinkStats } from "./LinkStats";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

function wrap(code: string) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return (
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[`/links/${code}`]}>
        <Routes><Route path="/links/:code" element={<LinkStats />} /></Routes>
      </MemoryRouter>
    </QueryClientProvider>
  );
}

describe("LinkStats", () => {
  beforeEach(() => { localStorage.setItem("quark_admin_token", "s"); vi.restoreAllMocks(); });

  it("mostra o total de cliques", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify({
      aggregates: { total: 42, first_ts: 1700000000, last_ts: 1700100000, per_day: { "2024-01-01": 42 }, per_country: { BR: 40, US: 2 }, per_device: { Mobile: 30, Desktop: 12 } },
      recent: [],
    }), { status: 200 }));
    render(wrap("6lB362J"));
    expect(await screen.findByText("42")).toBeInTheDocument();
  });

  it("estado vazio quando total 0", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify({
      aggregates: { total: 0, first_ts: 0, last_ts: 0, per_day: {}, per_country: {}, per_device: {} },
      recent: [],
    }), { status: 200 }));
    render(wrap("6lB362J"));
    expect(await screen.findByText(/sem cliques ainda/i)).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: implementar com frontend-design**

Invoque **`superpowers:frontend-design`** para construir a tela de stats (cartões + `StatsCharts` com Recharts + `RecentEventsTable`) satisfazendo o Contrato e fazendo o teste passar. Recharts em ambiente jsdom pode precisar de um wrapper com largura/altura fixas (`<div style={{width:600,height:300}}>`) pros gráficos renderizarem no teste — ou testar só os cartões/estado vazio (os testes acima já focam nisso).

- [ ] **Step 3: rodar + commit**

Run (em `web/`): `npm run test && npm run typecheck && npm run lint`

```bash
git add web/src
git commit -m "feat(web): tela de Stats por link (cartões + gráficos Recharts + eventos recentes + vazio)"
```

---

## Task 7: Tela de Blocklist  ·  **usa frontend-design**

**Files:** Create `web/src/routes/Blocklist.tsx`, `web/src/routes/Blocklist.test.tsx`; Modify `web/src/lib/queries.ts` (`useBlocklist` + mutations).

**Interfaces (Consumes):** `api.listBlocked/addBlocked/removeBlocked`.

**Contrato:**
- Rota `/blocklist`. Lista de domínios; input + botão **Adicionar**; cada item com **Remover** (com confirmação). Sucesso → toast + invalida.
- **Estados:** loading skeleton; vazio "nenhum domínio bloqueado"; erro recuperável.

**Checklist Nielsen:** confirmação no remover; feedback nas ações; vazio desenhado.

- [ ] **Step 1: teste de aceitação (falha primeiro)**

`web/src/routes/Blocklist.test.tsx`:
```tsx
import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { Blocklist } from "./Blocklist";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return <QueryClientProvider client={qc}>{ui}</QueryClientProvider>;
}

describe("Blocklist", () => {
  beforeEach(() => { localStorage.setItem("quark_admin_token", "s"); vi.restoreAllMocks(); });

  it("lista os domínios", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify({ domains: ["evil.com"] }), { status: 200 }));
    render(wrap(<Blocklist />));
    expect(await screen.findByText("evil.com")).toBeInTheDocument();
  });

  it("estado vazio", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify({ domains: [] }), { status: 200 }));
    render(wrap(<Blocklist />));
    expect(await screen.findByText(/nenhum domínio bloqueado/i)).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: implementar com frontend-design**

Invoque **`superpowers:frontend-design`** para construir a tela de Blocklist satisfazendo o Contrato e fazendo o teste passar.

- [ ] **Step 3: rodar + commit**

Run (em `web/`): `npm run test && npm run typecheck && npm run lint`

```bash
git add web/src
git commit -m "feat(web): tela de Blocklist (listar/adicionar/remover com confirmação + estados)"
```

---

## Task 8: CI de frontend + docs

**Files:** Modify `.github/workflows/ci.yml`, `README.md`.

- [ ] **Step 1: job `web` no CI**

Em `.github/workflows/ci.yml`, adicionar um job novo (irmão do `check`):

```yaml
  web:
    runs-on: ubuntu-latest
    defaults:
      run:
        working-directory: web
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: "20"
          cache: npm
          cache-dependency-path: web/package-lock.json
      - run: npm ci
      - run: npm run lint
      - run: npm run typecheck
      - run: npm run test
      - run: npm run build
```

- [ ] **Step 2: documentar no README**

Em `README.md`, na seção Operating (ou perto do docker-compose), adicionar:

```markdown
### Web panel (`web/`)

A single-operator admin panel (React SPA) lives in `web/`. It's built and
deployed **separately** from the API binary (static build → CDN/edge); the quark
binary stays API-only. Dev: `cd web && npm install && npm run dev` (Vite on
`:5173`), pointing `VITE_API_BASE_URL` at your quark API and setting
`QUARK_CORS_ORIGINS=http://localhost:5173` on the API. Auth is the same
`QUARK_ADMIN_TOKEN`, entered on the panel's login screen.
```

- [ ] **Step 3: rodar + commit**

Run (em `web/`): `npm run lint && npm run typecheck && npm run test && npm run build`
Expected: tudo verde.

```bash
git add .github/workflows/ci.yml README.md
git commit -m "ci+docs(web): job de frontend (lint/typecheck/test/build) + seção do painel no README"
```

---

## Self-Review (autor do plano)

**Cobertura da spec:**
- Stack (React/Vite/TS/shadcn/TanStack/Recharts/Vitest) → Task 1. ✓
- Cliente x-admin-token + VITE_API_BASE_URL + 401→logout → Task 2. ✓
- Auth token localStorage → Task 2. ✓
- Login (token→sonda) → Task 3. ✓
- Links (lista keyset, busca client-side, copiar, criar/editar/deletar+confirmação) → Tasks 4+5. ✓
- Stats (per_day/país/device Recharts + recentes + vazio) → Task 6. ✓
- Blocklist → Task 7. ✓
- Shell + sidebar + tema claro/escuro + toasts → Task 3. ✓
- Estados vazio/loading/erro + Nielsen → checklist em cada task de UI. ✓
- CI de frontend + docs → Task 8. ✓
- **frontend-design nas tasks de UI** → declarado no Método e em cada task 3–7. ✓

**Placeholders:** tasks de lógica/config/testes têm código completo; tasks de UI têm contrato + testes completos + diretiva frontend-design (adaptação consciente, documentada no Método).

**Consistência de tipos:** `api.*` (Task 2) usado por todos os hooks/telas; `Link`/`Stats`/`BlocklistResponse` idênticos ao shape da API do Tijolo 8; `role="searchbox"`/`getByLabelText(/token|url/i)`/nomes de botão ("Entrar","Criar") alinhados entre os testes e os contratos de implementação.

**Nota de escopo:** busca é client-side sobre o carregado (server-side fora); gráficos Recharts em jsdom podem exigir wrapper de tamanho — os testes focam cartões/estado/comportamento, não pixels.
