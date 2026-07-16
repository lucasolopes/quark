# End-to-end tests

Browser-level tests that run the real backend, the real redirect server, and a
real identity provider. They cover what unit and component tests cannot: the
full OIDC login handshake and the live 302 redirect.

## What runs where

```mermaid
flowchart LR
  PW[Playwright / Chromium] -->|:5173| Vite[Vite panel]
  Vite -->|VITE_API_BASE_URL :8080| Q[quark backend native]
  PW -->|OIDC login page :8081| KC[Keycloak test realm]
  Q -->|discover / token / jwks :8081| KC
```

- **Keycloak** runs in Docker on `:8081` with a seeded realm (`quark`), one admin
  user (`admin@quark.test` in group `quark-admins`) and one reader
  (`reader@quark.test` in `quark-readers`). Password for both: `password`.
- **quark** runs natively on `:8080` (started by `global-setup.ts`), OIDC pointed
  at Keycloak. Native, not in Docker, so the browser and the backend resolve the
  same issuer `http://localhost:8081/realms/quark`.
- **Vite** serves the panel on `:5173` with `VITE_API_BASE_URL=http://localhost:8080`.
  `:5173` and `:8080` are the same site (`localhost`), so the `SameSite=Lax`
  session cookie is sent cross-origin without a proxy.

## Run it

From the repo root, start the test IdP once:

```bash
docker compose -f docker-compose.e2e.yml up -d --wait
```

Build the backend (any profile), then run the suite:

```bash
cargo build
cd web
npm run e2e
```

`global-setup.ts` verifies Keycloak is reachable, starts a fresh quark with the
OIDC env, and waits for it; `global-teardown.ts` stops it. Vite is started by
Playwright's `webServer`.

Stop the IdP when done:

```bash
docker compose -f docker-compose.e2e.yml down
```

## Suites

- `oidc-login.spec.ts` — the full provider login: admin gets a full-scope
  session, reader gets read-only (and is refused a write), logout revokes.
- `token-flows.spec.ts` — the break-glass admin token reaches the panel, a
  created code redirects with a live 302, and the SSRF guard refuses an internal
  destination.
- `google-real.spec.ts` — a documented manual checklist for the real Google
  provider (skipped unless `QUARK_E2E_GOOGLE=1`; Google blocks automated login).
