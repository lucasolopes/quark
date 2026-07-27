**English** · [Português](CHANGELOG.PT_BR.md)

# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

Versioning is SemVer with 0.x semantics, the Cargo convention:

- `0.MINOR.0` is a breaking release. It can change the HTTP API, rename or
  remove a `QUARK_*` variable, or change the on-disk format.
- `0.MINOR.PATCH` is compatible features and bug fixes.
- `0.MINOR.0-rc.N` is a pre-release and never gets the `latest` image tag.

The public contract covered by those numbers is: the HTTP API (`/`, `/:code`,
`/:code/stats`, `/admin/*`), the `QUARK_*` variables, the LMDB on-disk format
and the Postgres migrations, and the webhook payload and signature. The Rust
library surface in `src/lib.rs`, the admin panel HTML, and the ClickHouse table
layout are not covered.

## [Unreleased]

## [0.4.1] - 2026-07-27

### Added

- **Workspace deletion.** `DELETE /admin/tenants/{id}` removes a workspace and
  everything scoped to it, in one transaction, plus its ClickHouse click events
  and its Keycloak realm. Only the `Owner` can delete, the last remaining
  workspace cannot be deleted, and a tenant the caller is not a member of
  answers `404` rather than `403` so the endpoint cannot be used to enumerate
  workspaces. The panel asks for the slug to be typed before it will proceed.
  Deleting frees the slug for reuse. See `docs/WORKSPACES.md` for what is
  removed immediately and what is removed eventually.
- **Workspace creation explains the wait.** The request provisions a realm, a
  client, a mapper and a user in Keycloak, so it is legitimately slow. The
  panel now says a sign-in is being prepared, and after a threshold adds that
  it is taking longer than usual and that reloading is safe.
- `disabled_reason` on the webhook representation in `GET /admin/webhooks`,
  which lets the panel tell a subscription the user paused from one the system
  disabled.

### Fixed

- **Webhook destination URLs no longer reach the logs.** For Discord, Slack,
  Telegram and the generic connectors the token lives in the URL path, so the
  URL is the credential. It was printed in full at ten log sites. The field is
  now a `WebhookUrl` whose `Display` prints host and port only, and `reqwest`
  does not accept it without an explicit `expose()`, so reintroducing the leak
  is a compile error rather than a review miss. The subscription's signing
  secret was also readable through the struct's `Debug`, and no longer is.
- **A dead webhook destination is no longer retried forever.** `404` and `410`
  mean the destination is gone, but every non-2xx response was treated the
  same, so a removed endpoint burned the full attempt budget on every event,
  indefinitely. Those two statuses now get one confirmation attempt, and a
  destination that fails it is disabled with the reason recorded and shown in
  the panel. `429`, `5xx`, timeouts and transport errors keep the existing
  backoff. `400` and `422` are deliberately not treated as permanent: they
  usually mean our payload is wrong, and disabling a customer's integration
  over our own bug is the worst outcome available.
- **Reconnecting Slack over a disabled subscription reactivates it.** The OAuth
  merge inherited `active: false` and the old reason, so the obvious fix
  (reconnect) appeared to work and delivered nothing.
- **Log events from `main.rs` carry fields again.** Eight sites built JSON by
  hand inside the log macro, which under `QUARK_LOG_FORMAT=json` produced a
  serialized object escaped inside `message`, so the values were text rather
  than queryable fields. Six of them logged errors at `info`, which no alert
  keyed on severity would ever match.

## [0.4.0] - 2026-07-26

### Changed

- **BREAKING: `QUARK_ACCESS_LOG` is gone.** The per-request access log now comes
  from tower-http's `TraceLayer` and is emitted at `DEBUG`, so it is off under
  the default `info` filter and turned on with `RUST_LOG=tower_http=debug`. A
  deployment that still sets `QUARK_ACCESS_LOG` will not fail, but it will no
  longer produce an access log. The new `QUARK_LOG_FORMAT=json` switches every
  log event to one JSON object per line, which is what a log pipeline wants.
- Errors are typed with `thiserror` across the crate, and logging goes through
  `tracing` instead of `eprintln!`. Nothing about the HTTP contract changes:
  handlers return the same statuses and the same short error bodies.
- Signing keys are held in `secrecy::SecretBox` so they are zeroized on drop and
  cannot be printed by accident.
- Dependencies: axum 0.7 to 0.8, heed 0.20 to 0.22, redis 0.27 to 1.x, sqlx 0.8
  to 0.9. No on-disk format, migration or wire format changes with them.

### Fixed

- **SSRF: `[::127.0.0.1]` was accepted as a destination.** The internal-address
  check existed in two copies that had drifted apart, and the link-creation one
  did not reject IPv4-compatible IPv6 addresses. There is now a single
  `is_internal_ip` covering IPv4-mapped and IPv4-compatible IPv6, CGNAT
  (100.64/10), `0.0.0.0/8`, multicast and the documentation ranges.
- The OIDC login `state` is compared in constant time.
- `POST /admin/logout` uses the shared CSRF guard instead of its own header
  check, so it accepts the same proofs every other state-changing endpoint does.
- A dropped click event (analytics channel full) is counted and logged instead
  of vanishing, so saturation is visible. The counter only runs on the drop
  path, so a healthy redirect pays nothing for it.
- OIDC configuration reads each required variable once and carries the value,
  rather than validating the variable and reading it again.

### Security

- `unsafe_code = "deny"` and a clippy policy (`unwrap_used`, `expect_used`,
  `panic`, `await_holding_lock`, `let_underscore_future`) are enforced in CI.
  The 28 remaining `expect()` in `src/` each carry a written justification.

## [0.3.1] - 2026-07-25

### Fixed
- OIDC login no longer crashes the server: the `jsonwebtoken` 10 upgrade shipped without a crypto backend, so validating any id_token panicked and restarted the process. The `rust_crypto` feature is now pinned and a canary test exercises a real JWT operation in CI so this class of regression fails the build instead of production.

## [0.3.0] - 2026-07-25

### Added
- Fully responsive admin panel (mobile, tablet, desktop): navigation drawer with hamburger on small screens, full-screen create/edit link dialogs on phones, per-screen reflow down to 360px wide, and 44px touch targets on primary controls.
- Local responsive QA script (`web/scripts/responsive-qa.mjs`): sweeps every screen across 4 breakpoints and both themes, failing on any horizontal overflow.

### Changed
- Production deploys are now release-driven: only version tags trigger a deploy, through a single release workflow.
- Major dependency upgrades: axum 0.8, ClickHouse client 0.15, chacha20poly1305 0.11, redis 1.4, plus React/Vite toolchain bumps.

### Fixed
- Stats charts no longer break when a tooltip label is not a string.

### Security
- OIDC id_token validation now requires the `exp`, `iss` and `aud` claims.

## [0.2.0] - 2026-07-24

First tagged release and first published container image. Everything below has
been in `main` since the project started; this entry marks the point where it
became installable.

### Added
- Short codes computed by a calibrated Feistel network with an ARX round
  function, a bijection over the id space with no code index kept on disk.
- Pluggable storage: embedded LMDB (default, zero-dependency) or Postgres for
  a multi-node, shared-database deployment.
- Pluggable cache: in-process by default, with an optional Valkey L2 tier and
  cross-node invalidation over Valkey pub/sub.
- Pluggable analytics: an embedded sink by default, or ClickHouse for an OLAP
  analytics backend; `GET /:code/stats` for aggregates and recent events.
- OIDC login (Authorization Code + PKCE) as an alternative to the admin token,
  with opaque revocable server-side sessions.
- Signed outgoing webhooks following the Standard Webhooks spec, on
  `link.created/updated/deleted/expired/clicked/broken/recovered`; a durable
  Postgres outbox with retry, backoff and dead-lettering, best-effort delivery
  on LMDB; Slack/Discord/Telegram notification channels built on the same
  subscription model.
- API tokens with scopes (`links_read`, `links_write`, `webhooks`,
  `analytics`, `full`) and an optional per-token rate limit.
- Redirect rules: per-link geo/device targeting, first match wins.
- A/B testing: weighted link variants with per-variant click stats.
- Deep linking: hosts the iOS `apple-app-site-association` and Android
  `assetlinks.json` files, plus device-aware redirect to an app destination.
- Password-protected links (argon2id), max-visits expiration with an optional
  fallback URL, and broken-link monitoring with webhook notifications on
  status transitions.
- Conversion forwarding to GA4 and Meta CAPI, dispatched off the redirect hot
  path.
- Importer for CSV/JSON exports from Bitly, Kutt, YOURLS and a generic format,
  with a partial-success per-row report.
- Tags, a UTM builder with locally saved templates, and server-side search on
  Postgres (client-side fallback on LMDB).
- Abuse protection on link creation: per-IP rate limiting and a built-in guard
  against internal/loopback network targets (SSRF).
- Admin panel (React, Vite, shadcn/ui, TanStack, Recharts): link CRUD, search,
  tags, QR codes, per-link stats, API token management.
- `docker-compose.yml` for a full local stack (quark, Postgres, Valkey,
  ClickHouse).
- `quark --version` and an `X-Quark-Version` header on `GET /health`.

### Security
- AGPL-3.0-only core with a CLA collected on every pull request.
- Private vulnerability reporting and a written security policy.

[Unreleased]: https://github.com/lucasolopes/quark/compare/v0.4.1...HEAD
[0.4.1]: https://github.com/lucasolopes/quark/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/lucasolopes/quark/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/lucasolopes/quark/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/lucasolopes/quark/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/lucasolopes/quark/releases/tag/v0.2.0
