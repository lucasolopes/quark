# Multi-tenancy P2b-frontend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the cloud panel the two screens that make multi-tenancy usable end-to-end — an onboarding "create workspace" gate for authenticated users without a workspace, and a header workspace switcher — consuming the P2b-backend endpoints. OSS is untouched.

**Architecture:** The React panel (`web/`) already routes through `RequireAuth` → `Shell` → route outlet. We extend the shared `me()` type with `memberships` + `current_tenant`, add `createWorkspace`/`switchWorkspace` to the API client and matching mutation hooks, then add: a reusable `CreateWorkspaceForm`; a full-screen `Onboarding` gate rendered by `RequireAuth` when a cloud user has no current workspace (with a `WorkspaceGate` that auto-switches when there's exactly one membership); and a `WorkspaceSwitcher` dropdown in the `Shell` header (with a create-workspace dialog). Switching or creating invalidates every query so per-tenant data reloads.

**Tech Stack:** React 19, TypeScript, Vite, TanStack Query, react-router-dom, Tailwind, Radix-based `components/ui/*` (Button/Card/Dialog/Input/DropdownMenu), sonner toasts, i18n via `@/i18n` (en + pt-BR), Vitest + Testing Library.

## Global Constraints

- Code and identifiers in English. Any user-facing copy goes through i18n (`en.ts` + `pt-BR.ts`), and prose follows the avoid-ai-writing rules (no em dashes, no AI-isms).
- **OSS parity:** in OSS the `me()` payload has no `memberships`/`current_tenant` fields. Neither the onboarding gate nor the switcher may ever appear in OSS, and the token (break-glass) login path renders the app unchanged. A test must assert OSS parity.
- No backend changes — the P2b-backend endpoints (`/admin/me`, `POST /admin/tenants`, `POST /admin/workspace/switch`) are merged and final.
- Match the existing panel style: reuse `components/ui/*`, the `Card`/`Dialog` patterns from `Login.tsx`/`CreateLinkDialog.tsx`, and the `useMutation`+`invalidateQueries` pattern from `lib/queries.ts`. Do not invent new visual design.
- Tests colocated as `*.test.tsx`/`*.test.ts`, using `withProviders` from `@/test-utils` and `vi.spyOn(globalThis, "fetch")` (see `Login.test.tsx`).
- Verification gate per task: `npm run typecheck`, `npm run lint` (oxlint, `--max-warnings 0`), `npm run test` (vitest run) all green — run from `web/`.

## Backend contract (already live — do not change)

- `GET /admin/me`
  - OSS / token-only: `{ authenticated: boolean, oidc_enabled: boolean, display?, scopes? }` — **no** `memberships`/`current_tenant`.
  - Cloud session: additionally `{ memberships: Array<{tenant_id: number, name: string, slug: string, role: "Owner"|"Admin"|"Member"|"Viewer"}>, current_tenant: number | null }`. `current_tenant` is null unless the session's tenant is one the user has a membership in (a fresh cloud login sits on tenant 0 with no membership → null).
- `POST /admin/tenants` body `{name: string, slug: string}` (cloud only; 404 in OSS): creates the tenant, grants the caller Owner, re-points the session at it. Returns 200 with the tenant JSON. Errors: 401 (no session), 409 (slug already exists), 429 (rate-limited), 503 (backend).
- `POST /admin/workspace/switch` body `{tenant_id: number}` (cloud only; 404 in OSS): validates the caller's membership, re-points the session. Returns 200 on success; 403 if no membership; 401 (no session); 503 (backend).

## File Structure

- `web/src/lib/types.ts` — add `Membership`, extend `MeResponse`.
- `web/src/lib/api.ts` — add `createWorkspace`, `switchWorkspace`.
- `web/src/lib/queries.ts` — add `useMe`, `useCreateWorkspace`, `useSwitchWorkspace`.
- `web/src/i18n/en.ts` + `web/src/i18n/pt-BR.ts` — add `onboarding.*` keys and `shell.*` switcher keys.
- `web/src/components/CreateWorkspaceForm.tsx` (+ test) — reusable name+slug form.
- `web/src/routes/Onboarding.tsx` (+ test) — full-screen gate view (workspace chooser + create form).
- `web/src/app/WorkspaceGate.tsx` — decides auto-switch (exactly one membership) vs render `Onboarding`.
- `web/src/app/RequireAuth.tsx` — wire the gate.
- `web/src/components/WorkspaceSwitcher.tsx` (+ test) — header dropdown with switch + create dialog.
- `web/src/app/Shell.tsx` — mount the switcher in the header.

---

### Task 1: Types, API client, query hooks, i18n

**Files:**
- Modify: `web/src/lib/types.ts:68`
- Modify: `web/src/lib/api.ts`
- Modify: `web/src/lib/queries.ts`
- Modify: `web/src/i18n/en.ts`, `web/src/i18n/pt-BR.ts`
- Test: `web/src/lib/api.test.ts` (append)

**Interfaces:**
- Produces:
  - `interface Membership { tenant_id: number; name: string; slug: string; role: string; }`
  - `MeResponse` extended with `memberships?: Membership[]` and `current_tenant?: number | null`.
  - `api.createWorkspace(name: string, slug: string): Promise<{ id: number; name: string; slug: string; created: number }>`
  - `api.switchWorkspace(tenantId: number): Promise<void>`
  - `useMe()` — `useQuery` on `["me"]`, `enabled: !hasToken()`, `retry: false`, `staleTime: 30_000`.
  - `useCreateWorkspace()` / `useSwitchWorkspace()` — mutations that invalidate **all** queries on success.

- [ ] **Step 1: Extend the types**

In `web/src/lib/types.ts`, replace the `MeResponse` line (currently line 68) with:

```ts
/** One workspace the current user belongs to (cloud only). */
export interface Membership { tenant_id: number; name: string; slug: string; role: string; }
/**
 * Response of `GET /admin/me`: current principal + whether OIDC is configured.
 * `memberships`/`current_tenant` are present only in cloud mode; their absence
 * means OSS (single-tenant), where the onboarding gate and switcher never show.
 * `current_tenant` is null when the session has no workspace selected yet.
 */
export interface MeResponse {
  authenticated: boolean;
  oidc_enabled: boolean;
  display?: string;
  scopes?: string[];
  memberships?: Membership[];
  current_tenant?: number | null;
}
```

- [ ] **Step 2: Add the API methods**

In `web/src/lib/api.ts`, inside the `api` object (after `logout`, before `createLink`), add:

```ts
  /** Creates a workspace (cloud only) and re-points the session at it. 409 if the slug is taken, 429 if rate-limited. */
  async createWorkspace(name: string, slug: string): Promise<{ id: number; name: string; slug: string; created: number }> {
    return jsonOrThrow(await req("/admin/tenants", { method: "POST", body: JSON.stringify({ name, slug }) }));
  },
  /** Switches the session's current workspace (cloud only). 403 if the user has no membership in `tenantId`. */
  async switchWorkspace(tenantId: number): Promise<void> {
    const res = await req("/admin/workspace/switch", { method: "POST", body: JSON.stringify({ tenant_id: tenantId }) });
    if (!res.ok) throw new ApiError(res.status, await res.text().catch(() => res.statusText));
  },
```

- [ ] **Step 3: Add the query hooks**

In `web/src/lib/queries.ts`, add an import for `hasToken` at the top:

```ts
import { hasToken } from "./auth";
```

Then add these hooks (place `useMe` near the top after `queryClient`, and the mutations wherever convenient):

```ts
/**
 * The `GET /admin/me` query, shared by `RequireAuth` and `WorkspaceSwitcher`.
 * Disabled when a break-glass token is present (OSS/token login never calls it),
 * so its data is undefined in that case and the switcher/gate stay hidden.
 */
export function useMe() {
  return useQuery({
    queryKey: ["me"],
    queryFn: () => api.me(),
    enabled: !hasToken(),
    retry: false,
    staleTime: 30_000,
  });
}

/**
 * Creates a workspace; the session is now scoped to it, so invalidate ALL
 * queries — every per-tenant list (links, stats, tokens, …) must reload, and
 * `["me"]` must refetch so the gate/switcher re-resolve.
 */
export function useCreateWorkspace() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({ name, slug }: { name: string; slug: string }) => api.createWorkspace(name, slug),
    onSuccess: () => { void client.invalidateQueries(); },
  });
}

/** Switches the current workspace; invalidates ALL queries (see `useCreateWorkspace`). */
export function useSwitchWorkspace() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (tenantId: number) => api.switchWorkspace(tenantId),
    onSuccess: () => { void client.invalidateQueries(); },
  });
}
```

- [ ] **Step 4: Add i18n keys (en)**

In `web/src/i18n/en.ts`, add an `onboarding` block (top-level, e.g. after `login`) and switcher keys inside `shell`:

```ts
  onboarding: {
    title: "Create your workspace",
    description: "A workspace holds your links and analytics. Name it to get started.",
    chooseTitle: "Choose a workspace",
    orCreate: "or create a new one",
    nameLabel: "Workspace name",
    namePlaceholder: "Acme",
    slugLabel: "Slug",
    slugHint: "Used in URLs and must be unique. Lowercase letters, numbers and dashes.",
    submit: "Create workspace",
    creating: "Creating…",
    slugTaken: "That slug is already taken. Pick another.",
    createError: "Could not create the workspace. Try again.",
  },
```

Inside the existing `shell` block, add:

```ts
    workspaceLabel: "Current workspace",
    switchWorkspace: "Switch workspace",
    createWorkspace: "Create workspace",
```

- [ ] **Step 5: Add i18n keys (pt-BR)**

In `web/src/i18n/pt-BR.ts`, mirror the same shape:

```ts
  onboarding: {
    title: "Crie seu workspace",
    description: "Um workspace guarda seus links e analytics. Dê um nome pra começar.",
    chooseTitle: "Escolha um workspace",
    orCreate: "ou crie um novo",
    nameLabel: "Nome do workspace",
    namePlaceholder: "Acme",
    slugLabel: "Slug",
    slugHint: "Usado nas URLs e precisa ser único. Letras minúsculas, números e hífens.",
    submit: "Criar workspace",
    creating: "Criando…",
    slugTaken: "Esse slug já está em uso. Escolha outro.",
    createError: "Não deu pra criar o workspace. Tente de novo.",
  },
```

And inside `shell`:

```ts
    workspaceLabel: "Workspace atual",
    switchWorkspace: "Trocar workspace",
    createWorkspace: "Criar workspace",
```

- [ ] **Step 6: Write API tests**

Append to `web/src/lib/api.test.ts` (follow the file's existing mocking style — inspect it first for the exact `fetch` spy / BASE handling; the assertions below are the intent):

```ts
describe("workspace endpoints", () => {
  it("createWorkspace posts name+slug to /admin/tenants and returns the tenant", async () => {
    const spy = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ id: 5, name: "Acme", slug: "acme", created: 1 }), { status: 200 }),
    );
    const t = await api.createWorkspace("Acme", "acme");
    expect(t.id).toBe(5);
    const [url, init] = spy.mock.calls[0];
    expect(String(url)).toContain("/admin/tenants");
    expect(init?.method).toBe("POST");
    expect(JSON.parse(String(init?.body))).toEqual({ name: "Acme", slug: "acme" });
  });

  it("createWorkspace throws ApiError(409) on a duplicate slug", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 409 }));
    await expect(api.createWorkspace("Acme", "acme")).rejects.toMatchObject({ status: 409 });
  });

  it("switchWorkspace posts tenant_id to /admin/workspace/switch", async () => {
    const spy = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 200 }));
    await api.switchWorkspace(7);
    const [url, init] = spy.mock.calls[0];
    expect(String(url)).toContain("/admin/workspace/switch");
    expect(JSON.parse(String(init?.body))).toEqual({ tenant_id: 7 });
  });

  it("switchWorkspace throws ApiError(403) when the user lacks membership", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 403 }));
    await expect(api.switchWorkspace(7)).rejects.toMatchObject({ status: 403 });
  });
});
```

- [ ] **Step 7: Verify**

Run from `web/`: `npm run typecheck && npm run lint && npm run test`. Expected: all green (new API tests pass; no type/lint errors).

- [ ] **Step 8: Commit**

```bash
git add web/src/lib/types.ts web/src/lib/api.ts web/src/lib/queries.ts web/src/i18n/en.ts web/src/i18n/pt-BR.ts web/src/lib/api.test.ts
git commit -m "feat(web): workspace types, API client + query hooks for P2b-frontend"
```

---

### Task 2: CreateWorkspaceForm component

**Files:**
- Create: `web/src/components/CreateWorkspaceForm.tsx`
- Test: `web/src/components/CreateWorkspaceForm.test.tsx`

**Interfaces:**
- Consumes: `useCreateWorkspace` (Task 1), `useT`, `ApiError`, `Button`/`Input` from `components/ui`.
- Produces: `export function CreateWorkspaceForm({ onCreated }: { onCreated?: () => void }): JSX.Element` — a name+slug form. On submit calls `createWorkspace`; on success calls `onCreated`. Shows the slug-taken message on 409, a generic error otherwise. Slug auto-derives from the name until the user edits the slug field.

- [ ] **Step 1: Write the failing test**

Create `web/src/components/CreateWorkspaceForm.test.tsx`:

```tsx
import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { CreateWorkspaceForm } from "./CreateWorkspaceForm";
import { withProviders } from "@/test-utils";

describe("CreateWorkspaceForm", () => {
  beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });

  it("derives the slug from the name and posts both", async () => {
    const spy = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ id: 1, name: "My Team", slug: "my-team", created: 1 }), { status: 200 }),
    );
    const onCreated = vi.fn();
    render(withProviders(<CreateWorkspaceForm onCreated={onCreated} />));
    await userEvent.type(screen.getByLabelText(/workspace name/i), "My Team");
    await userEvent.click(screen.getByRole("button", { name: /create workspace/i }));
    const init = spy.mock.calls.find((c) => String(c[0]).includes("/admin/tenants"))?.[1];
    expect(JSON.parse(String(init?.body))).toEqual({ name: "My Team", slug: "my-team" });
    expect(onCreated).toHaveBeenCalled();
  });

  it("shows the slug-taken message on 409 and does not call onCreated", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 409 }));
    const onCreated = vi.fn();
    render(withProviders(<CreateWorkspaceForm onCreated={onCreated} />));
    await userEvent.type(screen.getByLabelText(/workspace name/i), "Acme");
    await userEvent.click(screen.getByRole("button", { name: /create workspace/i }));
    expect(await screen.findByText(/slug is already taken/i)).toBeInTheDocument();
    expect(onCreated).not.toHaveBeenCalled();
  });

  it("shows a rate-limit message on 429", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 429 }));
    render(withProviders(<CreateWorkspaceForm />));
    await userEvent.type(screen.getByLabelText(/workspace name/i), "Acme");
    await userEvent.click(screen.getByRole("button", { name: /create workspace/i }));
    expect(await screen.findByText(/too many requests/i)).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `npm run test -- CreateWorkspaceForm` — Expected: FAIL (module not found).

- [ ] **Step 3: Implement the component**

Create `web/src/components/CreateWorkspaceForm.tsx`:

```tsx
import { Loader2 } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useT } from "@/i18n";
import { ApiError } from "@/lib/api";
import { useCreateWorkspace } from "@/lib/queries";

/** Lowercases, strips accents, and turns runs of non-alphanumerics into single dashes. */
function slugify(input: string): string {
  return input
    .normalize("NFD")
    .replace(/[̀-ͯ]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

/** Name+slug form to create a workspace. `onCreated` fires after a successful create. */
export function CreateWorkspaceForm({ onCreated }: { onCreated?: () => void }) {
  const t = useT();
  const [name, setName] = useState("");
  const [slug, setSlug] = useState("");
  const [slugEdited, setSlugEdited] = useState(false);
  const mutation = useCreateWorkspace();

  const effectiveSlug = slugEdited ? slug : slugify(name);

  function handleSubmit(e: React.FormEvent<HTMLFormElement>) {
    e.preventDefault();
    if (!name.trim() || !effectiveSlug || mutation.isPending) return;
    mutation.mutate(
      { name: name.trim(), slug: effectiveSlug },
      { onSuccess: () => onCreated?.() },
    );
  }

  const errorText =
    mutation.error instanceof ApiError && mutation.error.status === 409
      ? t("onboarding.slugTaken")
      : mutation.error instanceof ApiError && mutation.error.status === 429
        ? t("common.rateLimited")
        : mutation.isError
          ? t("onboarding.createError")
          : null;

  return (
    <form onSubmit={handleSubmit} className="flex flex-col gap-3" noValidate>
      <div className="flex flex-col gap-1.5">
        <label htmlFor="ws-name" className="text-sm font-medium">{t("onboarding.nameLabel")}</label>
        <Input
          id="ws-name"
          value={name}
          placeholder={t("onboarding.namePlaceholder")}
          onChange={(e) => setName(e.target.value)}
          autoFocus
        />
      </div>
      <div className="flex flex-col gap-1.5">
        <label htmlFor="ws-slug" className="text-sm font-medium">{t("onboarding.slugLabel")}</label>
        <Input
          id="ws-slug"
          value={effectiveSlug}
          onChange={(e) => { setSlugEdited(true); setSlug(slugify(e.target.value)); }}
          className="font-mono"
        />
        <p className="text-xs text-muted-foreground">{t("onboarding.slugHint")}</p>
      </div>
      {errorText && <p role="alert" className="text-sm text-destructive">{errorText}</p>}
      <Button type="submit" disabled={!name.trim() || !effectiveSlug || mutation.isPending} className="mt-1">
        {mutation.isPending && <Loader2 className="size-4 animate-spin" aria-hidden="true" />}
        {mutation.isPending ? t("onboarding.creating") : t("onboarding.submit")}
      </Button>
    </form>
  );
}
```

- [ ] **Step 4: Run the tests**

Run: `npm run test -- CreateWorkspaceForm` — Expected: PASS (3/3).

- [ ] **Step 5: Verify + commit**

Run from `web/`: `npm run typecheck && npm run lint && npm run test`. Then:

```bash
git add web/src/components/CreateWorkspaceForm.tsx web/src/components/CreateWorkspaceForm.test.tsx
git commit -m "feat(web): CreateWorkspaceForm (name+slug, slug auto-derive, 409/429 handling)"
```

---

### Task 3: Onboarding gate (WorkspaceGate + RequireAuth wiring)

**Files:**
- Create: `web/src/routes/Onboarding.tsx`
- Create: `web/src/app/WorkspaceGate.tsx`
- Modify: `web/src/app/RequireAuth.tsx`
- Test: `web/src/app/RequireAuth.test.tsx`

**Interfaces:**
- Consumes: `useMe`, `useSwitchWorkspace` (Task 1), `CreateWorkspaceForm` (Task 2), `Membership`/`MeResponse` types.
- Produces:
  - `export function Onboarding({ memberships }: { memberships: Membership[] }): JSX.Element` — full-screen view. When `memberships` is non-empty, shows a "choose a workspace" list (each row switches to that tenant); always shows the `CreateWorkspaceForm` below.
  - `export function WorkspaceGate({ me }: { me: MeResponse }): JSX.Element` — when exactly one membership, auto-switches to it (effect) and shows a spinner; otherwise renders `Onboarding`.
  - `RequireAuth` renders `WorkspaceGate` when authenticated + cloud + `current_tenant == null`.

- [ ] **Step 1: Write the failing test**

Create `web/src/app/RequireAuth.test.tsx`:

```tsx
import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { RequireAuth } from "./RequireAuth";
import { withProviders } from "@/test-utils";

function meResponse(body: object) {
  return new Response(JSON.stringify(body), { status: 200 });
}
const child = <div>APP CONTENT</div>;

describe("RequireAuth workspace gate", () => {
  beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });

  it("OSS (no memberships field) renders the app", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(meResponse({ authenticated: true, oidc_enabled: false }));
    render(withProviders(<RequireAuth>{child}</RequireAuth>));
    expect(await screen.findByText("APP CONTENT")).toBeInTheDocument();
  });

  it("cloud with a current workspace renders the app", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      meResponse({ authenticated: true, oidc_enabled: true, memberships: [{ tenant_id: 3, name: "Acme", slug: "acme", role: "Owner" }], current_tenant: 3 }),
    );
    render(withProviders(<RequireAuth>{child}</RequireAuth>));
    expect(await screen.findByText("APP CONTENT")).toBeInTheDocument();
  });

  it("cloud with zero memberships shows onboarding, not the app", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      meResponse({ authenticated: true, oidc_enabled: true, memberships: [], current_tenant: null }),
    );
    render(withProviders(<RequireAuth>{child}</RequireAuth>));
    expect(await screen.findByText(/create your workspace/i)).toBeInTheDocument();
    expect(screen.queryByText("APP CONTENT")).not.toBeInTheDocument();
  });

  it("cloud with exactly one membership and no current workspace auto-switches to it", async () => {
    const spy = vi.spyOn(globalThis, "fetch").mockImplementation((url, init) => {
      if (String(url).includes("/admin/workspace/switch")) return Promise.resolve(new Response("", { status: 200 }));
      // First /admin/me: no current; after the switch invalidates, still fine to return current set.
      return Promise.resolve(meResponse({ authenticated: true, oidc_enabled: true, memberships: [{ tenant_id: 9, name: "Solo", slug: "solo", role: "Owner" }], current_tenant: null }));
    });
    render(withProviders(<RequireAuth>{child}</RequireAuth>));
    await waitFor(() => {
      expect(spy.mock.calls.some((c) => String(c[0]).includes("/admin/workspace/switch") && JSON.parse(String(c[1]?.body)).tenant_id === 9)).toBe(true);
    });
  });

  it("cloud with two memberships and no current shows the chooser", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      meResponse({ authenticated: true, oidc_enabled: true, memberships: [
        { tenant_id: 1, name: "Acme", slug: "acme", role: "Owner" },
        { tenant_id: 2, name: "Beta", slug: "beta", role: "Member" },
      ], current_tenant: null }),
    );
    render(withProviders(<RequireAuth>{child}</RequireAuth>));
    expect(await screen.findByText(/choose a workspace/i)).toBeInTheDocument();
    expect(screen.getByText("Acme")).toBeInTheDocument();
    expect(screen.getByText("Beta")).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `npm run test -- RequireAuth` — Expected: FAIL (onboarding not wired; `Onboarding`/`WorkspaceGate` missing).

- [ ] **Step 3: Implement `Onboarding`**

Create `web/src/routes/Onboarding.tsx`:

```tsx
import { LanguageSwitcher } from "@/components/LanguageSwitcher";
import { CreateWorkspaceForm } from "@/components/CreateWorkspaceForm";
import { QuarkMark } from "@/components/brand/QuarkMark";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { useT } from "@/i18n";
import { useSwitchWorkspace } from "@/lib/queries";
import type { Membership } from "@/lib/types";

/**
 * Full-screen gate shown to a cloud user with no current workspace. With
 * existing memberships it lists them (pick one to switch); it always offers the
 * create-workspace form below. `RequireAuth` renders this; there is no route.
 */
export function Onboarding({ memberships }: { memberships: Membership[] }) {
  const t = useT();
  const switchWs = useSwitchWorkspace();
  const hasExisting = memberships.length > 0;

  return (
    <div className="flex min-h-svh items-center justify-center bg-background p-4">
      <div className="absolute right-4 top-4"><LanguageSwitcher /></div>
      <Card className="w-full max-w-sm">
        <CardHeader>
          <div className="mb-1 flex items-center gap-3">
            <QuarkMark className="size-8 text-primary drop-shadow-[0_0_10px_rgba(198,249,78,0.55)]" />
            <CardTitle className="font-heading text-2xl font-bold tracking-tight">
              {hasExisting ? t("onboarding.chooseTitle") : t("onboarding.title")}
            </CardTitle>
          </div>
          <CardDescription>{t("onboarding.description")}</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          {hasExisting && (
            <div className="flex flex-col gap-2">
              {memberships.map((m) => (
                <Button
                  key={m.tenant_id}
                  variant="outline"
                  className="justify-between"
                  disabled={switchWs.isPending}
                  onClick={() => switchWs.mutate(m.tenant_id)}
                >
                  <span className="truncate">{m.name}</span>
                  <span className="font-mono text-xs text-muted-foreground">{m.role}</span>
                </Button>
              ))}
              <div className="my-1 flex items-center gap-3 text-xs text-muted-foreground">
                <span className="h-px flex-1 bg-border" />
                {t("onboarding.orCreate")}
                <span className="h-px flex-1 bg-border" />
              </div>
            </div>
          )}
          <CreateWorkspaceForm />
        </CardContent>
      </Card>
    </div>
  );
}
```

- [ ] **Step 4: Implement `WorkspaceGate`**

Create `web/src/app/WorkspaceGate.tsx`:

```tsx
import { Loader2 } from "lucide-react";
import { useEffect } from "react";
import { Onboarding } from "@/routes/Onboarding";
import { useSwitchWorkspace } from "@/lib/queries";
import type { MeResponse } from "@/lib/types";

/**
 * Rendered by `RequireAuth` for a cloud user with no current workspace. With
 * exactly one membership it auto-switches into it (a returning single-workspace
 * user should not have to click); with zero or several it shows `Onboarding`.
 */
export function WorkspaceGate({ me }: { me: MeResponse }) {
  const memberships = me.memberships ?? [];
  const only = memberships.length === 1 ? memberships[0].tenant_id : null;
  const switchWs = useSwitchWorkspace();

  useEffect(() => {
    if (only != null && switchWs.isIdle) switchWs.mutate(only);
    // Fire once for the single-membership case; the mutation's own state guards re-entry.
  }, [only, switchWs]);

  if (only != null) {
    return (
      <div className="flex min-h-svh items-center justify-center bg-background">
        <Loader2 className="size-6 animate-spin text-muted-foreground" aria-label="Loading" />
      </div>
    );
  }
  return <Onboarding memberships={memberships} />;
}
```

- [ ] **Step 5: Wire `RequireAuth`**

Replace `web/src/app/RequireAuth.tsx` body to use `useMe` and the gate:

```tsx
import { Loader2 } from "lucide-react";
import type { ReactNode } from "react";
import { Navigate } from "react-router-dom";
import { useMe } from "@/lib/queries";
import { hasToken } from "@/lib/auth";
import { WorkspaceGate } from "./WorkspaceGate";

/**
 * Route guard. A saved break-glass token authenticates immediately. Otherwise
 * it checks for an OIDC login session (`GET /admin/me`); while that resolves it
 * shows a spinner, and with no token and no session it redirects to /login.
 * In cloud mode, an authenticated user with no current workspace is routed
 * through `WorkspaceGate` (onboarding / auto-switch) instead of the app.
 */
export function RequireAuth({ children }: { children: ReactNode }) {
  const tokenPresent = hasToken();
  const me = useMe();

  if (tokenPresent) return <>{children}</>;
  if (me.isLoading) {
    return (
      <div className="flex min-h-svh items-center justify-center bg-background">
        <Loader2 className="size-6 animate-spin text-muted-foreground" aria-label="Loading" />
      </div>
    );
  }
  if (!me.data?.authenticated) return <Navigate to="/login" replace />;
  // Cloud mode only (OSS omits `memberships`): gate on having a current workspace.
  const cloud = me.data.memberships !== undefined;
  if (cloud && me.data.current_tenant == null) return <WorkspaceGate me={me.data} />;
  return <>{children}</>;
}
```

- [ ] **Step 6: Run the tests**

Run: `npm run test -- RequireAuth` — Expected: PASS (5/5). If the auto-switch test flakes on query timing, assert only that the switch fetch fired (as written), not the post-switch render.

- [ ] **Step 7: Verify + commit**

Run from `web/`: `npm run typecheck && npm run lint && npm run test`. Then:

```bash
git add web/src/routes/Onboarding.tsx web/src/app/WorkspaceGate.tsx web/src/app/RequireAuth.tsx web/src/app/RequireAuth.test.tsx
git commit -m "feat(web): onboarding gate — WorkspaceGate auto-switch + Onboarding chooser/create"
```

---

### Task 4: WorkspaceSwitcher in the Shell header

**Files:**
- Create: `web/src/components/WorkspaceSwitcher.tsx`
- Modify: `web/src/app/Shell.tsx`
- Test: `web/src/components/WorkspaceSwitcher.test.tsx`

**Interfaces:**
- Consumes: `useMe`, `useSwitchWorkspace` (Task 1), `CreateWorkspaceForm` (Task 2), `DropdownMenu*` + `Dialog*` from `components/ui`, `useT`.
- Produces: `export function WorkspaceSwitcher(): JSX.Element | null` — returns `null` unless cloud (`me.memberships !== undefined`) with a `current_tenant`. Renders a dropdown trigger labelled with the current workspace name; the menu lists memberships (switch on select) and a "Create workspace" item that opens a dialog with `CreateWorkspaceForm`.

- [ ] **Step 1: Write the failing test**

Create `web/src/components/WorkspaceSwitcher.test.tsx`:

```tsx
import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { WorkspaceSwitcher } from "./WorkspaceSwitcher";
import { withProviders } from "@/test-utils";

function me(body: object) { return new Response(JSON.stringify(body), { status: 200 }); }
const cloudMe = {
  authenticated: true, oidc_enabled: true, current_tenant: 1,
  memberships: [
    { tenant_id: 1, name: "Acme", slug: "acme", role: "Owner" },
    { tenant_id: 2, name: "Beta", slug: "beta", role: "Member" },
  ],
};

describe("WorkspaceSwitcher", () => {
  beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });

  it("renders nothing in OSS (no memberships field)", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(me({ authenticated: true, oidc_enabled: false }));
    const { container } = render(withProviders(<WorkspaceSwitcher />));
    // Give the me() query time to resolve, then assert empty.
    await waitFor(() => expect(container).toBeEmptyDOMElement());
  });

  it("shows the current workspace and lists the others; selecting one switches", async () => {
    const spy = vi.spyOn(globalThis, "fetch").mockImplementation((url) =>
      String(url).includes("/admin/workspace/switch")
        ? Promise.resolve(new Response("", { status: 200 }))
        : Promise.resolve(me(cloudMe)),
    );
    render(withProviders(<WorkspaceSwitcher />));
    await userEvent.click(await screen.findByRole("button", { name: /acme/i }));
    await userEvent.click(await screen.findByText("Beta"));
    await waitFor(() => {
      expect(spy.mock.calls.some((c) => String(c[0]).includes("/admin/workspace/switch") && JSON.parse(String(c[1]?.body)).tenant_id === 2)).toBe(true);
    });
  });
});
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `npm run test -- WorkspaceSwitcher` — Expected: FAIL (module not found).

- [ ] **Step 3: Implement the component**

Create `web/src/components/WorkspaceSwitcher.tsx`:

```tsx
import { Check, ChevronsUpDown, Plus } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel,
  DropdownMenuSeparator, DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useT } from "@/i18n";
import { useMe, useSwitchWorkspace } from "@/lib/queries";

/**
 * Header control (cloud only) to switch between the user's workspaces and to
 * create a new one via a dialog. Returns null in OSS (`me.memberships`
 * undefined) or before a workspace is selected.
 */
export function WorkspaceSwitcher() {
  const t = useT();
  const me = useMe();
  const switchWs = useSwitchWorkspace();
  const [createOpen, setCreateOpen] = useState(false);

  const memberships = me.data?.memberships;
  const current = me.data?.current_tenant;
  if (!memberships || current == null) return null;
  const currentName = memberships.find((m) => m.tenant_id === current)?.name ?? "";

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="outline" size="sm" className="max-w-[12rem] justify-between gap-2">
            <span className="truncate">{currentName}</span>
            <ChevronsUpDown className="size-3.5 shrink-0 opacity-60" aria-hidden="true" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-56">
          <DropdownMenuLabel>{t("shell.workspaceLabel")}</DropdownMenuLabel>
          {memberships.map((m) => (
            <DropdownMenuItem
              key={m.tenant_id}
              disabled={switchWs.isPending || m.tenant_id === current}
              onSelect={() => { if (m.tenant_id !== current) switchWs.mutate(m.tenant_id); }}
            >
              <Check className={m.tenant_id === current ? "size-4" : "size-4 opacity-0"} aria-hidden="true" />
              <span className="truncate">{m.name}</span>
            </DropdownMenuItem>
          ))}
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={(e) => { e.preventDefault(); setCreateOpen(true); }}>
            <Plus className="size-4" aria-hidden="true" />
            {t("shell.createWorkspace")}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <Dialog open={createOpen} onOpenChange={setCreateOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("onboarding.title")}</DialogTitle>
            <DialogDescription>{t("onboarding.description")}</DialogDescription>
          </DialogHeader>
          <CreateWorkspaceFormLazy onCreated={() => setCreateOpen(false)} />
        </DialogContent>
      </Dialog>
    </>
  );
}

// Imported at the bottom to keep the component list readable; see Task 2.
import { CreateWorkspaceForm as CreateWorkspaceFormLazy } from "@/components/CreateWorkspaceForm";
```

Note: move the `CreateWorkspaceForm` import up with the other imports if oxlint flags import ordering — the trailing import above is illustrative only. Verify `DialogDescription` is exported by `components/ui/dialog`; if not, drop that line.

- [ ] **Step 4: Mount it in the Shell header**

In `web/src/app/Shell.tsx`, add the import and render the switcher first in the header (left of `LanguageSwitcher`):

```tsx
import { WorkspaceSwitcher } from "@/components/WorkspaceSwitcher";
```

Change the header opening so the switcher sits at the start and pushes the rest right:

```tsx
        <header className="flex h-14 shrink-0 items-center gap-2 border-b border-border px-4">
          <WorkspaceSwitcher />
          <div className="flex-1" />
          <LanguageSwitcher />
```

(Keep the theme + logout buttons as they are.)

- [ ] **Step 5: Run the tests**

Run: `npm run test -- WorkspaceSwitcher` — Expected: PASS (2/2).

- [ ] **Step 6: Verify + commit**

Run from `web/`: `npm run typecheck && npm run lint && npm run test` (full suite). Then:

```bash
git add web/src/components/WorkspaceSwitcher.tsx web/src/components/WorkspaceSwitcher.test.tsx web/src/app/Shell.tsx
git commit -m "feat(web): workspace switcher in the Shell header (switch + create dialog)"
```

---

## Self-Review

**Spec coverage:**
- Types + API (`MeResponse` + `createWorkspace`/`switchWorkspace`) → Task 1.
- Onboarding gate (cloud + authenticated + no current workspace) → Task 3 (`RequireAuth` + `WorkspaceGate` + `Onboarding`); 0 → create form, 1 → auto-switch, ≥2 → chooser (spec's "0 → onboarding; 1 → current; N → selector").
- Create-workspace screen → Task 2 (`CreateWorkspaceForm`, reused by onboarding + switcher dialog).
- Workspace switcher in header → Task 4.
- i18n EN+PT → Task 1. Vitest for gate/form/switcher → Tasks 2-4. OSS parity → Task 3 (OSS renders app) + Task 4 (switcher null in OSS).
- Risk mitigations: gate only fires when `memberships` present (cloud) → Task 3; token login bypasses via `hasToken()` short-circuit → Task 3; switch/create invalidate ALL queries → Task 1 hooks; `MeResponse` fields optional (compat) → Task 1.

**Placeholder scan:** the only soft spot is the `WorkspaceSwitcher` import-ordering note and the `DialogDescription` existence check — both call out a concrete verification the implementer performs against `components/ui/dialog`. No TBDs.

**Type consistency:** `Membership { tenant_id, name, slug, role }` and `MeResponse.{memberships?, current_tenant?}` defined in Task 1 are used identically in Tasks 3-4. Hook names (`useMe`, `useCreateWorkspace`, `useSwitchWorkspace`) and `api.createWorkspace`/`api.switchWorkspace` match across tasks.

## Verification (post-merge)

- Build the SPA (`npm run build`) and confirm no type errors.
- Manual/controller end-to-end against a cloud backend: fresh OIDC login → onboarding → create workspace → app loads scoped to it → header switcher lists it → create a second → switch between them → each shows its own links/analytics (FORCE RLS from P2a isolates them).
- OSS: token login and OSS OIDC render the app with no onboarding and no switcher.
