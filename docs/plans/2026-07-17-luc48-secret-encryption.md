# LUC-48 — Secret encryption at rest Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Encrypt the third-party secrets quark stores in Postgres — the per-tenant OIDC `client_secret` and the Sheets `refresh_token` — at rest, with an app-level AEAD key from env (`QUARK_ENCRYPTION_KEY`). Opt-in: unset env → plaintext (today's behavior); set → new writes encrypted, reads handle both. $0, no KMS.

**Architecture:** A `SecretBox` (XChaCha20-Poly1305, key from `QUARK_ENCRYPTION_KEY`) with `seal`/`open`. `PostgresStore` holds `Option<SecretBox>`. The oidc-blob and sheets store paths seal the secret field on write and open it on read; a `enc:v1:` prefix distinguishes ciphertext from legacy plaintext (passthrough). A boot backfill re-encrypts legacy rows when the key is set.

**Tech Stack:** Rust, `chacha20poly1305` crate (RustCrypto), `getrandom` (already a dep), base64. `src/codec.rs`/`src/permute.rs` UNTOUCHED.

## Global Constraints
- English; avoid-ai-writing. Opt-in: `QUARK_ENCRYPTION_KEY` unset → byte-for-byte today's plaintext behavior (a parity test proves it). OSS unaffected.
- Ciphertext format: `"enc:v1:" + base64(24-byte-nonce ‖ ciphertext)`. `open` on a value without the prefix returns it unchanged (legacy plaintext). Empty string stays empty (not encrypted).
- A decrypt failure (wrong key / corrupt) propagates as an error (visible login/sheets failure), never silently returns garbage.
- `src/codec.rs`/`src/permute.rs` MUST NOT be touched.
- PG-gated tests via `QUARK_TEST_DATABASE_URL` (local `postgres://quark:quark@localhost:5432/quark`), NON-SUPERUSER. `-j1`/`CARGO_BUILD_JOBS=1`; NO CONCURRENTLY. In Bash: `export PATH="$HOME/.cargo/bin:$PATH"`. LNK1104 = stale locked exe → kill+retry.
- Build/fmt/`clippy --all-targets -- -D warnings` clean.

## Seams (verified)
- `src/store/postgres.rs`: `OidcConfigBlob` (`:253-266`), free fns `oidc_config_blob(cfg)` (`:268`) + `row_to_oidc_config(r)` (`:284`) — client_secret is `b.client_secret`. Sheets: `put_sheets_connection` (`~:1693`) + `get_sheets_connection` (`~:1714`), `SheetsConnection.refresh_token`. `PostgresStore::open`/`open_with_replica` — where to build the `Option<SecretBox>`.
- `src/auth.rs`: `generate_token`/`generate_secret` use `getrandom` — mirror for the nonce.
- `src/store/lmdb.rs`: check whether it stores `sheets_connection` (OSS) — if so it needs the same treatment (or a passthrough since OSS won't set the key); confirm in Task 2.
- `src/main.rs`: boot backfill site (near the subdomain/keycloak backfills).
- `Cargo.toml`: add `chacha20poly1305 = "0.10"` + `base64` (check if base64 is already a dep; the codebase base64s signing keys — reuse whatever it uses).

## File Structure
- Create `src/secretbox.rs` (+ `pub mod secretbox;` in `src/lib.rs`).
- Modify `Cargo.toml`, `src/store/postgres.rs`, `src/store/lmdb.rs` (if needed), `src/main.rs`.
- Tests: `src/secretbox.rs` unit tests; extend `tests/oidc_config_it.rs` + a sheets gated test.

---

### Task 1: `SecretBox` module + dependency

**Files:** Create `src/secretbox.rs`; Modify `Cargo.toml`, `src/lib.rs`; unit tests in `src/secretbox.rs`.

**Produces:**
- `struct SecretBox` wrapping `XChaCha20Poly1305`.
- `SecretBox::from_env() -> Option<SecretBox>` (reads `QUARK_ENCRYPTION_KEY` base64→32 bytes; `None` + a one-line log if unset/invalid).
- `fn seal(&self, plaintext: &str) -> String` (empty→empty; else `enc:v1:base64(nonce‖ct)`).
- `fn open(&self, stored: &str) -> Result<String, SecretBoxError>` (prefix→decrypt; no prefix→Ok(stored.to_string()) legacy passthrough; decrypt fail→Err).
- Free helpers: `seal_opt(&Option<SecretBox>, &str) -> String` and `open_opt(&Option<SecretBox>, &str) -> Result<String, _>` (None→passthrough).

**Steps:**
- [ ] Check `Cargo.toml` for an existing base64 crate (the signing-key handling base64-decodes env); reuse it. Add `chacha20poly1305 = "0.10"`.
- [ ] Write failing unit tests: `seal` then `open` round-trips; two `seal`s of the same text differ (random nonce); `open` of a non-prefixed string returns it unchanged (legacy); `open` of an `enc:v1:` value under a DIFFERENT key errors; empty string seals/opens to empty; `seal_opt`/`open_opt` with `None` are passthrough.
- [ ] Run, confirm fail.
- [ ] Implement `secretbox.rs`. Nonce: 24 random bytes via `getrandom` (mirror `generate_token`). `from_env`: base64-decode the env, require exactly 32 bytes. `pub mod secretbox;` in `lib.rs`.
- [ ] Run unit tests; build/fmt/clippy. Commit `feat(secretbox): XChaCha20-Poly1305 seal/open keyed by QUARK_ENCRYPTION_KEY (opt-in)`.

---

### Task 2: Encrypt oidc client_secret + sheets refresh_token in the store

**Files:** Modify `src/store/postgres.rs` (+ `src/store/lmdb.rs` if it stores sheets); Test `tests/oidc_config_it.rs` + a sheets gated test.

**Steps:**
- [ ] Add `secretbox: Option<SecretBox>` to `PostgresStore`; build it via `SecretBox::from_env()` in `open` + `open_with_replica`. (Update any `PostgresStore` literal in tests if constructed directly; most tests use `open`.)
- [ ] Thread it through the oidc blob: change `oidc_config_blob(cfg, sb: &Option<SecretBox>)` to `seal_opt(sb, &cfg.client_secret)` for the stored `client_secret`; change `row_to_oidc_config(r, sb)` to `open_opt(sb, &b.client_secret)?`. Update the `&self` callers (`put_oidc_config`, `get_oidc_config`, `get_oidc_config_bare`) to pass `&self.secretbox`.
- [ ] Same for sheets: `put_sheets_connection` seals `refresh_token`, `get_sheets_connection` opens it. Check `src/store/lmdb.rs` — if it stores `sheets_connection`, either build a `secretbox` there too (OSS won't set the key → passthrough) or confirm OSS doesn't use it; keep OSS behavior unchanged.
- [ ] Write failing gated tests (`tests/oidc_config_it.rs`, sheets test): with `QUARK_ENCRYPTION_KEY` set (set it in the test via `std::env::set_var` before `open`, or a test ctor), `put_oidc_config` stores `blob->>'client_secret'` starting with `enc:v1:` (query the raw column), and `get_oidc_config_bare` returns the original plaintext; without the key → the raw column is the plaintext (parity). Legacy: write a row with a plaintext secret (no key), then read WITH the key → passthrough returns the plaintext; re-put → now `enc:v1:`. Same shape for sheets refresh_token.
- [ ] Run, confirm fail; implement; run gated + lib; build/fmt/clippy. Commit `feat(store): encrypt oidc client_secret + sheets refresh_token at rest (opt-in, legacy passthrough)`.

---

### Task 3: Boot backfill re-encrypt legacy + runbook

**Files:** Modify `src/main.rs` (backfill) + `src/store/postgres.rs` (a backfill helper if needed); Create `docs/RUNBOOK-secret-encryption.md`; test.

**Steps:**
- [ ] Add a store method or a `main.rs` pass (only when `secretbox` is set) that, for each oidc_config + sheets_connection whose secret lacks the `enc:v1:` prefix, re-writes it sealed. Idempotent (already-`enc:v1:` skipped). One-line log with the count. Mirror the shape of the existing keycloak/subdomain boot backfills.
- [ ] Gated test: seed a plaintext-secret row (key unset), then run the backfill WITH the key → the raw column becomes `enc:v1:`; running again → 0 changed (idempotent); the decrypted value is unchanged.
- [ ] Write `docs/RUNBOOK-secret-encryption.md`: generate a key (`head -c 32 /dev/urandom | base64`), set `QUARK_ENCRYPTION_KEY` in the Fly secrets, that losing/changing the key breaks existing ciphertext (rotation is a manual re-encrypt — flagged as a v1 limitation / follow-up), and that OSS/unset = plaintext.
- [ ] Run gated + full lib; build/fmt/clippy. Commit `feat(store): boot backfill re-encrypts legacy plaintext secrets + runbook (LUC-48)`.

## Verification (whole-plan)
- `secretbox` round-trips; legacy passthrough; wrong-key errors. Store seals/opens oidc client_secret + sheets refresh_token; raw column is `enc:v1:` with the key, plaintext without (parity). Backfill re-encrypts legacy, idempotent. OSS/unset unchanged. Full lib + gated (`oidc_config_it`, sheets) green; clippy `--all-targets -D warnings` clean. Opus review (secret-handling). Then whole-branch review before merge.
