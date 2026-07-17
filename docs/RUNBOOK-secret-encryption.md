# Secrets at rest (LUC-48): production runbook

quark can encrypt the two third-party secrets it stores in Postgres (the per-tenant OIDC `client_secret` and the Sheets connector's `refresh_token`) with an app-level key from `QUARK_ENCRYPTION_KEY`. Opt-in: leave the var unset and nothing changes, plaintext, same as before this feature existed.

## What's encrypted, and how

`src/secretbox.rs` wraps XChaCha20-Poly1305. `PostgresStore` builds an `Option<SecretBox>` from `QUARK_ENCRYPTION_KEY` at open time. When it's `Some`, the two fields above get sealed on write and opened on read; an `enc:v1:` prefix on the stored value marks it as sealed, so a value written before encryption was turned on still reads back correctly (passthrough).

This only applies to Postgres (cloud/self-host-with-Postgres). LMDB (single-node OSS) never touches this: it stores those fields plaintext regardless, because there's no `QUARK_ENCRYPTION_KEY` handling on that backend at all.

## Turning it on

### 1. Generate a key

```bash
head -c 32 /dev/urandom | base64
```

That's a random 32-byte key, base64-encoded: exactly what `QUARK_ENCRYPTION_KEY` expects. `SecretBox::from_env` rejects anything that doesn't decode to exactly 32 bytes (logs a warning and falls back to plaintext rather than crashing).

### 2. Set it as a Fly secret

```bash
fly secrets set QUARK_ENCRYPTION_KEY="<the base64 value>" -a quark-prod
```

This restarts the app with the new env var. Treat this key the same way you treat `QUARK_KEY`/`QUARK_SIGNING_KEY`: a secret, never in version control, never logged.

### 3. The boot backfill does the rest

On every boot where `QUARK_ENCRYPTION_KEY` is set, quark scans `oidc_configs` and `sheets_connection` for any row whose secret is still plaintext and seals it in place. This runs automatically; there's no separate migration command to run. It's idempotent (a row already sealed is skipped) and cheap (a handful of rows, once per boot), so it's safe on every replica in a multi-node deployment.

The boot log has one line for it:

```
secret re-encryption backfill: <n> re-encrypted
```

`n` is 0 on every boot after the first pass. That's expected, not a failure.

New writes (a tenant setting up OIDC, or connecting Sheets, after the key is live) are sealed immediately by `put_oidc_config`/`put_sheets_connection`; the backfill only deals with rows that predate the key.

## Losing or changing the key

The key isn't recoverable from the ciphertext. If `QUARK_ENCRYPTION_KEY` is lost, every `enc:v1:`-prefixed value in the database becomes unreadable. `SecretBox::open` fails closed (an authentication error) rather than returning garbage, so this shows up as OIDC logins failing and Sheets sync erroring out, not as silent corruption. Back up the key the same way you'd back up `QUARK_KEY`: outside the database, somewhere your secrets manager or password vault reaches.

Changing the key (rotation) has the same effect: existing `enc:v1:` values were sealed under the old key, and the new `SecretBox` can't open them. **There is no automatic rotation in this version.** Rotating today means, for every affected row, decrypting with the old key and re-encrypting with the new one by hand (or a one-off script built for the occasion) before switching `QUARK_ENCRYPTION_KEY` over. This is a known v1 limitation, tracked as a follow-up. Don't attempt it casually in production without a tested script and a backup.

## OSS / self-host without the key

Unset `QUARK_ENCRYPTION_KEY` (the default) and everything behaves exactly as it did before LUC-48: `client_secret` and `refresh_token` are stored plaintext, on both LMDB and Postgres. There's no forced migration, no format change, nothing to opt out of.
