# HTTP layer: handlers, errors, guard, router

## Handler shape

Every route handler in `src/api/*.rs` follows this exactly: 68 `-> Response`
sites, and zero handlers returning `Result`.

```rust
pub(crate) async fn admin_links_patch(
    State(st): State<Arc<AppState>>,   // always first
    Path(code): Path<String>,          // then Path
    Query(p): Query<ListParams>,       // then Query / RawQuery / Option<ConnectInfo<SocketAddr>>
    headers: HeaderMap,                // then headers
    body: Bytes,                       // body-consuming extractor is always LAST (axum requires it)
) -> Response {
    // ...
    StatusCode::NO_CONTENT.into_response()
}
```

- `-> Response` with `.into_response()` on every branch. Never
  `Result<_, E>`, never `impl IntoResponse`, never `?` inside a handler.
- There is **no `impl IntoResponse` for any error type** in the whole crate
  (`src/api/mod.rs:23` is the only mention of the trait, a re-export). Do not
  introduce an `AppError`.
- Every file in `src/api/` starts with `use super::*;`. Shared imports
  (`StatusCode`, `Response`, `IntoResponse`, `Json`, `StoreError`, `header`,
  `HeaderMap`, `State`, `Path`, `Ordering`, ...) are `pub(crate) use` at the top
  of `src/api/mod.rs:18-32`. If a new type is needed by two submodules, add the
  re-export in `mod.rs` rather than importing it per file.
  Evidence: `src/api/domains.rs:1`, 12 of 12 submodules.

## Authorization

Not a middleware, not an extractor. A function called as the first real
statement of every admin handler.

```rust
// after the multi_tenant gate, before touching the store
let p = match admin_guard(&st, &headers, Scope::LinksWrite).await {
    Ok(p) => p,
    Err(status) => return status.into_response(),
};
```

- `admin_guard(&AppState, &HeaderMap, Scope) -> Result<Principal, StatusCode>`.
  It returns `StatusCode`, **not `Response`**, on purpose: keeps the `Err`
  variant `Copy` and small, avoiding clippy's `result_large_err`. Documented at
  `src/api/guard.rs:15-18`.
- Use `p.tenant` to scope every store call. Never a hardcoded tenant.
- Credential order inside the guard: `QUARK_ADMIN_TOKEN` (constant-time, always
  `Scope::Full` + `DEFAULT_TENANT`, no store access) -> API token from
  `x-admin-token` by hash -> `qk_session` cookie, honored only if
  `st.oidc_configured`. It tries **all** credentials and resolves the error at
  the end in a fixed precedence: 503 > 429 > 403 > 401/404
  (`src/api/guard.rs:62-70,158-167`). Do not early-return on the first failure.
- Cloud (`multi_tenant`): scopes are re-derived per request from
  `get_membership(user_id, tenant_id)` + `tenant::role_scopes(role)`. Never trust
  `session.scopes` in cloud (`src/api/guard.rs:110-150`).

### Choosing the Scope

`crate::auth::Scope` has exactly five variants, serde `snake_case`: `LinksRead`,
`LinksWrite`, `Webhooks`, `Analytics`, `Full`. `covers` is flat: `Full` covers
everything, everything else covers only itself - `LinksWrite` does **not** cover
`LinksRead` (asserted at `src/auth.rs:124-125`).

| Endpoint kind | Scope |
|---|---|
| GET link / tag / folder | `Scope::LinksRead` |
| POST / PATCH / DELETE link | `Scope::LinksWrite` |
| webhook CRUD, test send | `Scope::Webhooks` |
| analytics reads | `Scope::Analytics` |
| tenancy, domains, invites, tokens, OIDC config | `Scope::Full` |

Role mapping lives in `tenant::role_scopes` (`src/tenant.rs:102-111`):
Owner/Admin = Full, Member = Write+Read+Analytics, Viewer = Read+Analytics. So
changing an endpoint's scope changes which cloud roles can call it.

### CSRF

`csrf_guard(&headers) -> Result<(), StatusCode>` (403), called **after**
`admin_guard`, and only on state-changing endpoints reachable by a "simple"
cross-site POST - meaning no required JSON body. Four call sites:
`src/api/links.rs:650`, `src/api/sheets.rs:237,313`,
`src/api/webhooks_api.rs:556`.

```rust
if let Err(status) = csrf_guard(&headers) { return status.into_response(); }
```

It requires `x-admin-token` OR `x-quark-csrf` (`HEADER_ADMIN_TOKEN` /
`HEADER_CSRF`, `src/api/mod.rs:103-105`). Rationale: a custom header forces a
CORS preflight, which the explicit allowlist blocks. Those two headers are
exactly the `allow_headers` of the `CorsLayer` (`src/api/router.rs:239-259`),
which uses an explicit list and never `Any`, because `allow_credentials(true)`
is required for the split-origin session cookie.

`src/api/oidc_login.rs:359-362` does a stricter manual check instead. That is the
outlier; use `csrf_guard`.

## Status code contract

| Situation | Status | Body |
|---|---|---|
| Store call failed | 503 `SERVICE_UNAVAILABLE` | none |
| Store write hit a unique constraint | `conflict_or_503(e)` -> 409 or 503 | none |
| Resource does not exist | 404 | none |
| Feature/surface disabled (`!st.multi_tenant`, admin unconfigured) | 404 | none |
| Validation failed | 4xx | `&'static str`, short, lowercase, no punctuation |
| Third-party HTTP call failed (IdP, Slack, Sheets) | 502 `BAD_GATEWAY` | short `&'static str` |
| Backend does not support the operation | 501 `NOT_IMPLEMENTED` | none |
| Genuinely local failure (password hash, client build) | 500 | none |

Histogram that establishes the convention: 107 `SERVICE_UNAVAILABLE` vs 4
`INTERNAL_SERVER_ERROR` in `src/api/*.rs`; 97 `(StatusCode::X, "literal")`
tuples vs 185 bare statuses; 22 `!st.multi_tenant -> 404` gates.

Optional reads always match all three arms - never collapse `None` into `Err`:

```rust
let rec = match st.store.get_link(p.tenant, id).await {
    Ok(Some(v)) => v,
    Ok(None) => return StatusCode::NOT_FOUND.into_response(),
    Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
};
```

The disabled-surface gate is the **first** line of the handler, before the guard:

```rust
if !st.multi_tenant { return StatusCode::NOT_FOUND.into_response(); }
```

A disabled surface must not reveal that it exists (`src/api/guard.rs:52-60`).

### Error bodies never carry internals

Error message bodies are stable `&'static str` literals. Never put
`e.to_string()` from sqlx/reqwest/heed in a response body - the cause goes to the
JSON `eprintln!`. The panel discriminates on `err.status`, never on body content
(`web/src/lib/mutation-error.ts:11-24`), so changing a status is a contract
change: update the matching `assert_eq!(resp.status(), ...)` in `tests/*_it.rs`.

RFC 9457 `problem+json` is deliberately **not** used. Do not introduce it.

## Error types

Use `#[derive(thiserror::Error)]` with `#[non_exhaustive]`. Full rules, the
conversion table for the ten hand-written enums still in the repo, and how to
retire the ~20 `Result<_, String>` signatures: see
[errors-and-observability.md](errors-and-observability.md).

What stays true regardless of the error type:

- Driver errors (sqlx, redis, clickhouse) are flattened at the adapter into
  `Backend(String)` rather than carried with `#[from]`. That keeps those crates
  out of `quark::store`'s semver surface and the `Err` small on the redirect hot
  path (the same concern documented at `src/api/guard.rs:16-18`).
- Local 1-to-1 causes (heed, serde_json) do use `#[from]`.
- The variant set is what the handler acts on: `UniqueViolation` exists so a
  write can answer 409 instead of 503. Keep enums small and per-module; never
  merge them into one `AppError`.
- The error type never reaches the wire. The handler matches the variant and
  returns a status plus a stable `&'static str`.

### Core logic shared by two callers

When one core function serves handlers with different output shapes, the error
enum is the seam and HTTP stays out of the core:

```rust
// src/api/links.rs:290-303  enum, no StatusCode inside
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CreateError { /* ... */ }

// :437-454  for HTTP
fn create_error_response(err: CreateError) -> Response { /* ... */ }
// :458-469  for the import summary string
fn create_error_reason(err: &CreateError) -> &'static str { /* ... */ }
```

Validation helpers used by several handlers return
`Result<(), (StatusCode, &'static str)>` or `Result<(), Response>`, consumed with
`if let Err(..) = helper(..) { return ...; }` - never `?`
(`src/api/links.rs:194-205,226-249`).

## Router

```rust
Router::new()
    .route("/:code", get(redirect))
    .route("/admin/links", get(admin_links_list).post(admin_link_create))
    .route("/admin/links/:code", axum::routing::delete(admin_link_delete))
    // ... one single chain
    .with_state(state)   // exactly once, at the end
```

- All routes in one chain, `.with_state(state)` once at the end
  (`src/api/router.rs:206`), `.layer(...)` only after that.
- **Path params are `:code`, not `{code}`.** This is axum 0.7. `{}` syntax
  panics at startup here. Migrating to 0.8 is a dedicated task (Host moves to
  axum-extra, handlers require `Sync`, `Option<Path<T>>` changes meaning).
- Verbs beyond `get`/`post` use the full path: `axum::routing::delete(...)`,
  `.patch(...)` (`src/api/router.rs:96-99`).
- Optional layers use shadowing: `let app = if cond { app.layer(..) } else { app };`
- `router(state)` reads the env (`QUARK_CORS_ORIGINS`, `QUARK_ACCESS_LOG`) and
  delegates to `router_with_cors(state, origins)`, which takes config explicitly.
  That pair is the test seam - keep it.
- Middleware is `async fn f(req: Request, next: Next) -> Response` registered with
  `axum::middleware::from_fn`. Extract the pure part (log formatting) into a
  testable function with no IO: `access_log_line(...)` at
  `src/api/router.rs:19-28`.

## Request / response types

serde structs declared inline in the submodule that owns the route, not in a
shared types module.

```rust
#[derive(Deserialize)]
pub(crate) struct AlertReq { /* ... */ }

#[derive(Serialize)]
pub(crate) struct LinkRow {
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
}
```

Naming: `Req` / `Resp` suffix (abbreviated - `Request`/`Response` would collide
with axum's re-exported `Response`); `Row` / `Info` / `View` for read shapes;
`ListParams` for query structs. Handlers are `admin_<resource>_<verb>`
(`admin_links_list`, `admin_domains_verify`). Routes are kebab-case and plural
(`/admin/sso-domains`), sub-actions as a final segment
(`/admin/domains/:id/verify`).

`#[serde(skip_serializing_if = "Option::is_none")]` on every `Option` is what
justifies the `?` optional fields in `web/src/lib/types.ts`. There is no type
generation: the contract is written by hand twice. Renaming a Rust field compiles
and passes `cargo test` while silently breaking the panel.

`#[serde(deny_unknown_fields)]` is used nowhere - unknown body fields are ignored
on purpose.

## PATCH semantics

Partial update reads raw `serde_json::Value`, not a struct of `Option`s, because
it must distinguish absent (leave alone) from `null` (clear):

```rust
// src/api/links_admin.rs:450,466-518
let patch: serde_json::Value = match serde_json::from_slice(&body) {
    Ok(v) => v,
    Err(_) => return (StatusCode::BAD_REQUEST, "invalid json").into_response(),
};
if let Some(x) = patch.get("folder") {
    if x.is_null() { rec.folder = None; }
    else if let Some(s) = x.as_str() { rec.folder = Some(s.to_string()); }
    else { return (StatusCode::BAD_REQUEST, "invalid folder").into_response(); }
}
```

## Resolving `:code` in an admin handler

Never decode by hand. `resolve_for_admin(&st, tenant, &code)` handles base62 +
`permute::MAX_ID` and falls back to alias lookup scoped to the tenant's default
domain (`src/api/links_admin.rs:248-263`). To produce a code, use
`st.encode_code(id)` (`src/api/mod.rs:112-114`). Any valid base62 decodes to
*some* id, so existence and ownership are proven by the following
`get_link(p.tenant, id)`, not by the decode.

## Logging from the request path

- Inside a handler the error is discarded: `Err(_) =>` and map to a status. Do
  not log a store error per request - that is a hot-path cost and the ratio in
  `src/api/*.rs` reflects it (~107 `Err(_)` vs ~25 `Err(e)`). This survives the
  move to `tracing`: keep the redirect path free of per-request events.
- Logging with the error attached belongs to workers, background tasks and boot,
  via `tracing::error!(error = %e, ...)`.
- Best-effort side effects inside a handler are ignored with `let _ = ...` or
  `.ok();`.
- The access log is a `TraceLayer`, not a hand-rolled middleware. The legacy
  `access_log_line` + `println!` at `src/api/router.rs:19-44` and the
  `QUARK_ACCESS_LOG` gate go away with it - `RUST_LOG` subsumes the gate.
