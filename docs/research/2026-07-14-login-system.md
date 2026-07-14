# Login system for quark (OSS self-hosters)

Status: research / decision-support. Not a spec.
Date: 2026-07-14.
Scope: the open-source, single-tenant product. Cloud/SaaS multi-tenant is out of scope here.

## The question

Today a self-hoster logs into the quark panel by pasting one shared secret, `QUARK_ADMIN_TOKEN`. Scripts use scoped API tokens (`src/auth.rs`: `qtok_` prefix, SHA-256 hashed, per-scope). There are no user accounts, no sessions, no OAuth. Both the panel and the API authenticate the same way: an `x-admin-token` header checked in `admin_guard` / `require_admin_for_create` (`src/api.rs`).

That is fine for one operator. It stops being fine the moment a self-hoster wants more than one person to log in, wants to know who did what, or wants to offboard someone without rotating a secret everyone shares. The question this doc answers: what should quark ship so a self-hoster can have real accounts and a proper login, without dragging in a mandatory external service and breaking the single-binary, zero-required-dependency ethos that the pluggable backends already follow (minimal default, opt-in upgrade, selected at startup by env var).

Short answer up front: keep the token as the zero-dependency default, add an opt-in OIDC client so self-hosters point quark at their own identity provider, and treat a built-in username/password store as a later, lower-priority option only if demand shows up. The reasoning follows.

## What quark actually needs

Two different audiences use the auth surface, and they want opposite things:

- Machines (CI, scripts, integrations) want a long-lived bearer credential in a header. The current API tokens already serve this well and should not change.
- Humans using the panel want a login they do not have to paste from a password manager on every device, ideally tied to an account with a name attached to it.

So the target is: leave the machine path alone, and give the human path a real session-based login that can be backed either by the self-hoster's existing identity provider or, at most, by a small built-in credential store. Anything that forces every self-hoster to run a second server fails the ethos test.

## Options

### 1. Status quo: shared admin token plus scoped API tokens

What it is: one `QUARK_ADMIN_TOKEN` for full access, plus named `qtok_` tokens with scopes (`links_read`, `links_write`, `blocklist`, `webhooks`, `analytics`, `full`). Panel login is pasting the token.

- Self-hostable: yes, it is the binary.
- License / dependency: none added. This is the zero-dependency baseline.
- Cost in Rust/axum: zero, already built.
- When it is enough: a single operator, or a small trusted team that is comfortable sharing one secret and rotating it by hand. No audit trail, no per-person revocation for the panel (API tokens can be revoked individually, the shared admin token cannot without rotating it for everyone).

This stays as the default. The whole argument below is about what to add next to it, not what to replace it with.

### 2. Built-in username and password with server-side sessions

What it is: quark stores users (with an Argon2id password hash) and issues a session cookie after a login form. Self-contained in the binary, no external service.

The pieces you take on:
- Password hashing with the `argon2` crate, Argon2id, which is the current OWASP recommendation and the Password Hashing Competition winner (https://docs.rs/argon2/, https://cheatsheetseries.owasp.org/cheatsheets/Password_Storage_Cheat_Sheet.html).
- Session storage and a session cookie. `tower-sessions` (https://docs.rs/tower-sessions/) is the standard axum session layer; `axum-login` (https://docs.rs/axum-login/) sits on top for the user/auth-session plumbing. The `Store` trait already gives quark a place to persist a `users` table and a `sessions` table in LMDB or Postgres, so the storage seam exists.
- Cookie security: `HttpOnly`, `Secure`, `SameSite`, and session-id rotation on login to avoid session fixation.
- CSRF protection for cookie-authenticated mutations, which the current header-token model does not need (a bearer header is not sent automatically by the browser; a cookie is).
- The parts nobody wants to own: password reset (which pulls in an email dependency, or a manual CLI reset), account lockout / login rate limiting, and a first-user bootstrap flow.

- Self-hostable: yes, fully in-binary.
- License / dependency: `argon2`, `tower-sessions`, maybe `axum-login`, all permissive Rust crates, no external service. Preserves the single-binary ethos.
- Cost in Rust/axum: moderate. The hashing and sessions are small. The cost is the surrounding lifecycle (reset, lockout, bootstrap, CSRF) and owning credential security forever.

Verdict: technically clean and it keeps the ethos, but it makes quark responsible for storing and protecting passwords, which is exactly the responsibility OIDC lets a self-hoster hand to software built for it. Worth it only if a meaningful number of self-hosters want multiple accounts and refuse to run any identity provider. Rank it below OIDC.

### 3. OIDC / OAuth2, bring your own identity provider

What it is: quark acts as an OIDC client (relying party). The self-hoster points it at whatever provider they already run or already use: Google, GitHub, or a self-hosted Authelia / Authentik / Keycloak / Zitadel / Ory. quark never stores a password. It redirects to the provider, gets an ID token back, verifies it, and maps the verified identity to a quark principal with scopes.

This is the standard self-hosted pattern, and it fits the pluggable-backend model exactly: off by default, turned on by setting env vars (issuer URL, client id, client secret), no new service bundled into quark itself. The self-hoster who wants real accounts already has, or is willing to run, an identity provider; the one who does not keeps the token path.

Rust support is real and current:
- `openidconnect` (https://docs.rs/openidconnect/) and `oauth2` (https://docs.rs/oauth2/) handle the token exchange and ID-token verification.
- `axum-oidc` (https://codeberg.org/pfzetto/axum-oidc, MPL-2.0) is an axum-specific client with `OidcAuthLayer` (auth optional) and `OidcLoginLayer` (auth required) layers; latest release 1.0.0-dev-2 dated May 2026, and it recently dropped its `tower-sessions` dependency. Useful as a reference even if quark wires the lower-level crates directly to keep control of the session model.
- `oauth-kit` (https://github.com/cachix/oauth-kit) is a newer batteries-included OAuth/OIDC client with normalized profiles and plug-and-play axum integration.

The self-hostable providers a quark user might point at, compared on weight and license (state of open-source identity, 2025: https://blog.houseoffoss.com/post/the-state-of-open-source-identity-in-2025-authentik-vs-authelia-vs-keycloak-vs-zitadel):

| Provider | Runtime / weight | Extra services | License | Notes |
|---|---|---|---|---|
| Authelia | Go, very light | none required for basic use (file or SQLite backed) | Apache-2.0 | Started as a forward-auth companion; now also a standards OIDC provider. Lightest option for a small self-hoster. |
| Authentik | Python | Postgres + Redis | MIT core, separate enterprise license | Flow-engine, flexible, popular sweet spot for small teams. Heavier to run. |
| Keycloak | Java (JVM) | database | Apache-2.0 | Battle-tested enterprise IAM, Red Hat backed, CNCF. Heaviest. |
| Zitadel | Go single binary | database | AGPL-3.0 since the v3 release in 2025 (was Apache-2.0), commercial license available | Event-sourced, built for multi-tenancy. The 2025 relicense to AGPL matters for anyone embedding, not for quark as a plain client (https://skycloak.io/blog/open-source-authentication-comparison-2026/). |
| Ory Kratos + Hydra | Go microservices | database, multiple services | Apache-2.0 | Composable and powerful, but it is a multi-service stack to orchestrate. Overkill as a dependency for a URL shortener. |

quark does not pick one of these and does not bundle any of them. It speaks OIDC and lets the self-hoster choose. That is the whole point: the dependency stays on the self-hoster's side of the line, opt-in, and quark's binary is unchanged.

- Self-hostable: yes, and it does not force quark itself to grow a service. The self-hoster supplies the provider.
- License / dependency in quark: permissive Rust client crates only. No mandatory external service in the default build.
- Cost in Rust/axum: moderate. Authorization-code flow with PKCE, JWKS verification, a session after login, and a mapping from provider claims to quark scopes. The mapping (which verified emails or groups get `full` vs a narrower scope) is the design work, not the protocol.

Verdict: this is the recommended upgrade path. It gives real accounts and an audit-able identity without quark storing credentials, and it respects the minimal-default / opt-in pattern.

### 4. Embeddable auth libraries and services for the SPA

The panel is a React SPA talking to the axum backend, not a Next.js app, which rules out most of the popular JavaScript auth kits by construction:

- Auth.js / NextAuth (https://authjs.dev/): free and open-source, but it expects a JavaScript server runtime and a database adapter. quark's server is Rust. It would mean standing up a separate Node process, which breaks the single-binary story. Not a fit.
- Lucia: deprecated in March 2025 and reframed as a learning resource for implementing sessions from scratch rather than a library to install (https://github.com/lucia-auth/lucia, https://github.com/lucia-auth/lucia/discussions/1714). It is also TypeScript. Do not adopt it; do read it as a reference for the session model if quark builds option 2.
- Clerk (https://clerk.com/): SaaS, no self-hosting option, user data on Clerk's infrastructure. Disqualified for an OSS self-hosted tool.
- Supabase Auth (https://supabase.com/): open-source and self-hostable, but self-hosting it means running GoTrue plus Postgres plus the surrounding Supabase stack, and extracting auth later is nontrivial lock-in. Too heavy to require for a shortener.
- Ory: covered above under OIDC providers; usable as a bring-your-own provider, not something to embed.

The takeaway: the JavaScript-ecosystem auth kits assume a JavaScript backend or a hosted service. For a Rust single-binary tool the right shape is a Rust client (option 3) or a small in-binary store (option 2), with the SPA just driving a redirect and reading session state, not importing an auth SDK that expects its own server.

### 5. Magic links and passkeys (WebAuthn)

Modern passwordless options, both usable but with caveats for a self-hoster:

- Magic links: email a one-time login URL. Removes passwords, but makes email a hard dependency (an SMTP config or a mail provider). For a tool a self-hoster may run without any outbound mail set up, that is a real cost. Reasonable as an option, not as a baseline.
- Passkeys / WebAuthn: `webauthn-rs` (https://docs.rs/webauthn-rs/) is a maintained Rust server library, and the guidance is to use a maintained library rather than hand-rolling the cryptographic verification. Passkeys are a strong, phishing-resistant second step or primary factor. They still need a user record to attach the credential to, so they layer on top of option 2 or a hybrid, rather than replacing the account model. Good as a later enhancement once accounts exist.

Neither is a starting point. Both are things to offer after the account model is in place.

## Recommendation

Ship in stages. Each stage is opt-in and leaves the previous one working.

Stage 0 (today, keep it): shared admin token plus scoped API tokens. This stays the default and the zero-dependency path. A self-hoster who does nothing gets exactly what they have now. The machine/API path never changes.

Stage 1 (build next): opt-in OIDC client, bring your own identity provider. Env-gated the same way the backends are (for example `QUARK_OIDC_ISSUER`, `QUARK_OIDC_CLIENT_ID`, `QUARK_OIDC_CLIENT_SECRET`, plus a claim-to-scope mapping). When unset, quark behaves exactly as today. When set, the panel offers a "Log in with your provider" flow, and a verified identity maps to a quark principal with scopes. This is the real-accounts answer for anyone who runs, or is willing to run, any identity provider, and it never makes quark store a password.

Stage 2 (only if demand shows up): a lightweight built-in username/password store with sessions, for self-hosters who want multiple accounts but refuse to run any provider. Argon2id, `tower-sessions`, users and sessions in the existing `Store` trait, CSRF on cookie mutations. Defer it because it makes quark own credential security and the password-reset lifecycle, which OIDC avoids. Passkeys via `webauthn-rs` and magic links belong here too, as enhancements once an account model exists.

Rationale in one line: OIDC gives the biggest jump in capability (real, named, revocable accounts) for the least permanent liability (no stored passwords) while staying opt-in and single-binary, so it comes before the built-in credential store, not after.

## Implementation sketch

The plug point is `admin_guard` (`src/api.rs`). Today it reads `x-admin-token` and checks it against the env token or a hashed API token, then checks the scope. Extend it to accept either credential, without disturbing the header path:

1. If a session cookie is present and valid, resolve it to a principal and its scopes. Use that.
2. Otherwise fall back to the existing `x-admin-token` check (env token or `qtok_` API token). Unchanged, so every script and the current panel keep working.
3. Apply the same `Scope::covers(required)` check regardless of which credential authenticated the request.

So both credentials converge on the same principal-plus-scopes shape, and the abstraction the rest of the handlers see does not change.

Session versus JWT for the panel: prefer a server-side opaque session id in an `HttpOnly` cookie over a self-contained JWT. A stored session is revocable immediately (log someone out, kill a stolen session) and needs no signing-secret management, at the cost of one store lookup per request, which is cheap next to what the panel already does. Keep bearer JWTs, if any, out of the panel path; the API keeps its `qtok_` bearer tokens, which are already opaque and hashed. The session table lives behind the `Store` trait, same as everything else, so LMDB single-node and Postgres multi-node both work (a shared session store also means a session stays valid across nodes).

OIDC flow (stage 1): authorization-code with PKCE. quark redirects to the issuer with `state` and `nonce`, receives the code on a fixed redirect path, exchanges it for tokens using `openidconnect` / `oauth2`, verifies the ID token against the issuer's JWKS (signature, `iss`, `aud`, `exp`, and the `nonce`), reads the verified subject/email/groups, maps them to a quark principal and scopes, then creates a server-side session and sets the cookie. The mapping config is the real decision surface: decide which verified claim value grants `full` and which grants a narrower scope, and default closed (an authenticated user with no matching mapping gets nothing).

## Security must-dos

- OIDC: PKCE on the authorization-code flow; validate `state` (CSRF on the callback) and `nonce` (replay); verify the ID token signature via JWKS and check `iss`, `aud`, and `exp`; trust only verified emails/claims; default the claim-to-scope mapping closed.
- Cookies: `Secure`, `HttpOnly`, `SameSite=Lax` (or `Strict` for the panel), TLS on every authenticated request and not just the login page.
- Sessions: rotate the session id on login (anti session-fixation), server-side and revocable, with an idle and absolute timeout.
- CSRF: because cookie auth is sent automatically by the browser, guard state-changing panel requests with a CSRF defense (SameSite plus a custom header the SPA sets, or a double-submit token). The header-token API path does not need this and should stay as is.
- If stage 2 ships: Argon2id for password hashing with a per-password random salt; login rate limiting and lockout (quark already has a rate limiter in `abuse`); a safe first-user bootstrap; and a password-reset path that does not silently require outbound email the operator has not configured.
- Keep the API-token model unchanged: opaque `qtok_`, SHA-256 hashed at rest, shown once, individually revocable.

## Open questions for the owner

- Is stage 1 (OIDC) enough on its own for the OSS product, or is there real demand for accounts without any external provider (which is what forces stage 2)?
- For OIDC, how should claims map to quark's scopes? A single admin group to `full`, or a full mapping from provider groups to each scope? Where does the mapping live: env, a config file, or the store?
- Should the shared `QUARK_ADMIN_TOKEN` remain a permanent break-glass credential even after OIDC is on, or be disable-able once real accounts exist?
- Multi-user implies "who did what." Is a per-principal audit log in scope for the OSS product, or deferred to cloud?
- Which OIDC providers get first-class documented setup guides (Authelia and Google/GitHub seem the highest-value starting pair for self-hosters)?
- Does the panel need more than one role (for example a read-only viewer using `links_read` plus `analytics`), and should those roles be nameable in the UI?
- Session storage on LMDB single-node: acceptable to keep sessions in the same environment, or should they be memory-only there to avoid write amplification?

## Sources

- https://github.com/lucia-auth/lucia
- https://github.com/lucia-auth/lucia/discussions/1714
- https://blog.houseoffoss.com/post/the-state-of-open-source-identity-in-2025-authentik-vs-authelia-vs-keycloak-vs-zitadel
- https://wz-it.com/en/blog/authentik-vs-zitadel-identity-provider-comparison/
- https://skycloak.io/blog/open-source-authentication-comparison-2026/
- https://github.com/ory/kratos
- https://codeberg.org/pfzetto/axum-oidc
- https://docs.rs/openidconnect/
- https://docs.rs/oauth2/
- https://github.com/cachix/oauth-kit
- https://docs.rs/tower-sessions/
- https://docs.rs/axum-login/
- https://docs.rs/argon2/
- https://cheatsheetseries.owasp.org/cheatsheets/Password_Storage_Cheat_Sheet.html
- https://docs.rs/webauthn-rs/
- https://authjs.dev/
- https://clerk.com/
- https://supabase.com/
- https://blog.vibecoder.me/clerk-vs-authjs-vs-supabase-auth
