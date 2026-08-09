# Security, crypto, multi-tenancy

The primitive set is deliberately minimal: `hmac`, `sha2`, `chacha20poly1305`,
`argon2`, `jsonwebtoken`, `getrandom`, `base64`. Do not add `subtle`,
`constant_time_eq` or `rand` - the repo already has what they provide.

`secrecy` and `zeroize` are the exception: they are **required** for anything
holding key material, and they are not primitives but leak protection. See
[errors-and-observability.md](errors-and-observability.md#secrecy-and-zeroize).

## Constant-time comparison

```rust
// src/api/router.rs:3-12  - the project's own 10-line helper
if !constant_time_eq(provided.as_bytes(), expected.as_bytes()) { /* reject */ }
```

Use `constant_time_eq` for any secret coming from a request (admin token, OAuth
`state`). For MACs use `mac.verify_slice(&provided)` from `hmac` - decode the
base64 header first and pass raw bytes; never generate the signature locally,
format it, and compare two `String`s.

Short-circuiting `==` leaks the correct prefix by timing, and the admin token is
the break-glass path that runs before any per-credential rate limit.

Known inconsistency: the OIDC callback compares `state` with `!=`
(`src/api/oidc_login.rs:169-172`) while Sheets and Slack use `constant_time_eq`.
The latter is dominant and correct.

## Randomness

```rust
let mut buf = [0u8; 32];
getrandom::fill(&mut buf).expect("system RNG must be available");
```

`getrandom::fill` is the only CSPRNG. **Never add `rand`** -
`src/password.rs:1-8` documents that the salt comes from `getrandom` specifically
to avoid pulling `rand` in for a salt.

- Security path: `.expect("system RNG must be available")` - that exact string, 7
  production uses (two sites use a different wording; prefer the dominant one).
- Best-effort path (backoff jitter, click id): handle the error with
  `is_ok()`/`is_err()` and degrade without panicking.

## Opaque tokens

Every opaque credential (API token, session, invite, domain verification token)
comes from `crate::auth::generate_token()` and is persisted **only** as
`hash_token(&plaintext)` (SHA-256 hex via `crate::hex`). The plaintext is returned
exactly once, in the creation response, and never again. Do not write another
generator and never store plaintext.

Note: `generate_token` maps random bytes with `b as usize % 62`, so the first 8
alphabet characters are ~1.6% more likely. At 32 characters the token still has
~190 bits of entropy, so this is an observation, not an exploitable bug, and
changing the alphabet would invalidate existing token shapes. For a **new**
secret, either use rejection sampling (discard `b >= 248`) or hex/base64 of the
raw buffer.

## secretbox: versioned envelope

Third-party secrets at rest (per-tenant OIDC client_secret, Sheets refresh
token) go through `secretbox::seal_opt` / `open_opt`. Format:
`enc:v2:<keyid>:<b64(nonce||ct)>`, XChaCha20Poly1305, fresh 24-byte nonce per
seal, `keyid = hex(SHA-256(key)[..4])`.

**Always pass the AAD via the store's dedicated helper**, never inline and never
`b""`:

```rust
secretbox::seal_opt(sb, secret, &aad_oidc_client_secret(tenant))  // "<tenant_id>:oidc_client_secret"
```

The `AAD_OIDC_CLIENT_SECRET` / `AAD_SHEETS_REFRESH_TOKEN` constants exist so seal
and open cannot diverge. Empty AAD loses the per-row binding: a v2 ciphertext
copied to another row or tenant would still decrypt. A test locks that a different
AAD yields `DecryptFailed`.

Keyring: primary from `QUARK_ENCRYPTION_KEY`, decrypt-only olds from the
comma-separated `QUARK_ENCRYPTION_KEY_OLD`. Every key is base64 of exactly 32
bytes; an invalid primary means `None` (encryption off, warning on stderr), an
invalid old is ignored with a warning. `open` passes plaintext through when there
is no known prefix (pre-LUC-48 compatibility) and **never brute-forces keys** on
an `enc:v2:` value - an unknown keyid is `UnknownKey`. Never write the legacy
`enc:v1:` format on a new seal (v1 carries no AAD and exists for read
compatibility only).

Scope limit, stated in the module doc: secretbox is used **only** in
`src/store/postgres.rs`. LMDB does not reference it, `reencrypt_legacy_secrets`
defaults to `Ok(0)` on the trait, and webhook signing secrets and pixel
credentials are not encrypted on any backend. Do not claim broader coverage.

## Passwords

Link passwords: `Argon2::default()` (argon2id), stored as a PHC string in
`Record.password_hash`, 16-byte salt from `getrandom` via
`SaltString::encode_b64`. `verify_password` **never panics** - a malformed PHC
returns `false` (fail closed). Verification runs in `spawn_blocking` and the
endpoint is rate-limited.

The unlock cookie is `"<expiry>.<b64url(HMAC(key, code.expiry.password_hash))>"`.
Binding the current password hash into the MAC is what makes outstanding unlock
tokens die on password rotation - there is a test
(`unlock_token_after_password_rotation_is_rejected`). Never sign only
`code.expiry`.

`argon2` stays on 0.5: the 0.6 line is still `-rc`. `Argon2::default()` in 0.5.3
already matches the OWASP minimum (m=19456 KiB, t=2, p=1); do not invent
parameters and never lower memory.

## Standard Webhooks

`webhooks::sign(secret, msg_id, ts, body)` over `"{msg_id}.{ts}.{body}"`,
HMAC-SHA256, emitted as `v1,<base64>`. The secret must be `whsec_<base64>` and the
key is the base64-decoded suffix.

Reject both a missing prefix and an empty decoded key
(`SignError::EmptyOrMalformedSecret`) - HMAC with an empty key is a fixed,
guessable key. The three headers `webhook-id`, `webhook-timestamp`,
`webhook-signature` are sent **only** for `SubscriptionKind::Generic`; native
channels (Slack/Discord/Telegram) go unsigned because the secret URL is the
authentication, and there are tests asserting the headers are absent. The official
test vector is locked in a unit test.

When receiving a webhook, verify over the **raw body bytes** before
deserializing: extract `axum::body::Bytes`, verify, then `serde_json::from_slice`.
Never reserialize the JSON between signing and sending.

## SSRF

Two layers, and they are not interchangeable.

1. **String-only guard, on every user-supplied destination URL** (link target,
   A/B variant, app deep link, webhook URL, custom domain host):
   `extract_host` + `is_internal_host` (`src/abuse/mod.rs:6-43`). On the link path
   also `is_blocked_target`, which adds anti-self-loop (`public_host`, the request
   `Host`, and in multi-tenant any resolved custom domain). This guard
   **deliberately does not resolve DNS**.
2. **When the server itself makes the request**, use `health::safe_to_probe`,
   which resolves with `lookup_host` under a timeout and rejects if **any**
   resolved address is internal - and rejects when nothing resolves
   (`src/health.rs:74-138`).

Passing `is_internal_host` is *not* a licence to fetch: a public name pointing at
`169.254.169.254` or an RFC1918 address gets through the string guard.

All outbound clients that touch a user URL set
`.redirect(reqwest::redirect::Policy::none())`.

Known inconsistency: `abuse::is_internal_host` (dominant, 9 call sites) does not
cover `is_documentation()` (192.0.2.0/24 etc.) or IPv4-compatible IPv6
(`::a.b.c.d`), while `health::is_internal_ip` (1 call site) covers both. So
`[::127.0.0.1]` is blocked by the health checker but passes link creation. If you
harden one, harden both, or factor the `IpAddr` classification into a single
helper in `src/abuse/mod.rs` that `health` reuses. Do not rely on
`IpAddr::is_global` - still unstable.

Table-driven unit tests for the guard should cover `::ffff:127.0.0.1`,
`[::1]`, `100.64.0.1`, `169.254.169.254`, and a hostname resolving to loopback.

## Rate limiting

`RateLimiter` has exactly three modes - `disabled` / `memory` / `valkey` - chosen
in `main` from `QUARK_RATELIMIT_PER_MIN` (0 = off, the default) and the presence
of `QUARK_VALKEY_URL`. Fixed window: `now_secs / WINDOW_SECS` with
`const WINDOW_SECS: u64 = 60`. Valkey key is always
`quark:rl:{key}:{window}` with EXPIRE = 2 windows on the first INCR.

A Valkey error **fails open** (lets the request through) by design. Use
`check(ip, now)` for the global limit and `check_with_limit(key, now, limit)` for
a per-token quota (key `format!("tok:{}", token.id)`).

## Client IP

`client_ip(&headers, &st.real_ip_header, conn.as_ref())`. The header comes from
`QUARK_REAL_IP_HEADER`, default `DEFAULT_REAL_IP_HEADER = "cf-connecting-ip"`.
Fallback is the socket IP, last resort the literal `"unknown"` (one conservative
bucket). The project **does not parse `X-Forwarded-For`**. For the cookie `Secure`
decision use `request_is_https`, which reads `x-forwarded-proto` and takes the
first entry.

## Cookies

- Session: `qk_session={raw}; Max-Age=SESSION_TTL_SECS; Path=/; HttpOnly;
  SameSite=None; Secure` under HTTPS (needed for the split-origin panel), and
  `SameSite=Lax` on plain HTTP (`None` without `Secure` is invalid).
- Flow cookies (`qk_login`, `qk_sheets_state`, `qk_slack_state`):
  `Max-Age=600; Path=/; HttpOnly; SameSite=Lax` + conditional `Secure`, cleared
  with `Max-Age=0` after use. All those responses carry `Cache-Control: no-store`.
- To emit two `Set-Cookie` headers use `headers_mut().append`, never an array of
  tuples (that overwrites).

## OIDC

`verify_id_token` uses `Validation::new(Algorithm::RS256)` with `set_issuer`,
`set_audience(client_id)` and `validate_exp = true`, then applies two checks the
crate does not do:

- `azp`, when present, must equal our client_id - and a multi-audience token
  **without** `azp` is rejected. Otherwise a token minted for another client that
  merely lists us in `aud` would be accepted.
- `nonce` must match the login nonce, or the code could be replayed.

Errors are classified `VerifyError::BadSignature` (the only retryable one -
triggers exactly one JWKS refetch) vs `Rejected` (final, no refetch), so a burst
of bad logins cannot hammer the IdP's `jwks_uri`. Only RSA JWKs are accepted
(`DecodingKey::from_rsa_components`); a token without `kid` only works if there is
exactly one key. Never use `insecure_disable_signature_validation`; tests sign
real tokens.

Login state travels in one HMAC-signed cookie:
`sign_login_state(key, state, verifier, nonce, tenant)` ->
`"state.verifier.nonce.tenant.mac"`, verified with `verify_login_state` /
`mac.verify_slice`. **The tenant comes only from that cookie, never from a query
param** - a client-supplied tenant on the callback would redirect validation to
another IdP. There is a test that a tampered tenant field (`.42.` -> `.43.`)
fails the MAC. Sheets and Slack reuse the same functions, storing the tenant in
the `verifier` slot.

Claim mapping: `claim_contains` matches **exactly** (string or array), never
substring - there is a test that `acme-contractors-alumni` does not pass a gate
requiring `acme-contractors`. OSS/global `map_scopes` is default-closed: only
`admin_value` grants `Full`, `readonly_value` grants Read+Analytics, everything
else gets an empty vector (403 "no quark access"). Cloud per-tenant `claim_role`
always resolves some role (default `Role::Member`) and admission is decided by
`passes_required_group`, checked **before** creating a membership or session.
`Role::Owner` never comes from a claim (only from creating a workspace or
accepting an invite) and a login never downgrades an existing Owner.

## Tenant isolation in handlers

Never read the tenant from body, query or header, and never hardcode
`DEFAULT_TENANT` in an admin handler. Take `p.tenant` from `admin_guard` and pass
it as the first argument of every Store method. Switching workspace validates
`get_membership` first (403 if absent). Post-read ownership comparison is the
anti-pattern; the scope goes into the query.

In cloud, `admin_guard` re-derives scopes from the membership in the *current*
tenant rather than trusting the stored `session.scopes`, because those were minted
at login possibly for another tenant.

## Tenant slug

Validate with `tenant::is_valid_slug` **before** creating a Tenant or calling
Keycloak: `^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$`, 1..=63 chars, and reject
`RESERVED_SLUGS` case-insensitively. The slug goes verbatim into a Keycloak realm
name, into Admin API paths, into the derived OIDC issuer and into the
auto-provisioned subdomain. It is **immutable by contract** (LUC-51) - do not add
a rename route.

## permute is obfuscation, not a MAC

Short codes come from `permute::obfuscate(id, key)` - a 6-round Feistel over 40
bits from the `arxid` crate (extracted from this repo; spec v2), re-exported as
`quark::permute`. The key is a `u64` parsed from `QUARK_KEY` as decimal; when
absent it falls back to the development value `0x9E3779B97F4A7C15` while
printing "DO NOT use in production".

Treat it as anti-enumeration obfuscation, **not** a cryptographic secret.
`obfuscate`/`deobfuscate` mask the input (`& MAX_ID`), are total, and never
panic. **Never reuse `QUARK_KEY` to sign anything.**

Signing uses a separate secret: `st.signing_key: [u8; 32]`, base64 of
`QUARK_SIGNING_KEY` truncated to the first 32 bytes, requiring `len() >= 32`.
Without it, `main` generates a per-process random key and warns that cookies die
on restart and are not shared across nodes. Never reuse another var for this and
never fail over silently.

## Never leak a secret or a resource's existence

- Persisted secrets never come back on a read: webhook secrets become
  `mask_secret` (`"whsec_••••"`), pixel credentials become
  `MASKED_SECRET = "••••"` via `mask_credentials`, `OidcConfigView` omits
  `client_secret` (and a PUT with an empty secret **preserves** the stored one).
- A persisted error detail uses `e.without_url().to_string()`, because a channel
  webhook URL embeds the secret and reqwest's `Display` includes the URL - the
  detail is returned by `GET /admin/webhooks`. The full string may only go in the
  immediate response body to the owning admin, and in the stderr log.
- Enumeration: every failure of `/admin/login?org=<slug>` returns the identical
  404 from `org_login_not_found()`, and the endpoint is IP rate-limited to close
  the timing side channel too.
- `AppState` does not derive `Debug` (`src/api/mod.rs:34`), so there is no path
  for a `{state:?}` to dump `signing_key` / `admin_token` / `client_secret`. That
  absence is load-bearing but fragile: it holds only until someone adds the
  derive. **Key material must be `secrecy::SecretBox` / `SecretString`**, whose
  `Debug` prints `[REDACTED]` and whose `Drop` zeroes the memory, so the
  protection survives a careless derive. Applies to `AppState::signing_key`
  (`src/api/mod.rs:41`) and to every key in `src/secretbox.rs`.
  **Never derive `Debug` on a type containing key material** even so - `secrecy`
  makes the mistake survivable, not acceptable.
- With `panic = "abort"`, destructors do not run on abort - do not rely on `Drop`
  to clear key material.

## Do not "fix" these

Two fail-open behaviours are explicit product decisions, documented in the code:
a Valkey error in `check_with_limit` returns `true` (request passes), and
`deliver_one` exhausting its attempt budget only logs. Changing either is a
product decision, not a bugfix.
