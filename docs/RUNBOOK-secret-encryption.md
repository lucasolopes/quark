# Secrets at rest (LUC-48): production runbook

quark can encrypt the two third-party secrets it stores in Postgres (the per-tenant OIDC `client_secret` and the Sheets connector's `refresh_token`) with an app-level key from `QUARK_ENCRYPTION_KEY`. Opt-in: leave the var unset and nothing changes, plaintext, same as before this feature existed.

## What's encrypted, and how

`src/secretbox.rs` wraps XChaCha20-Poly1305. `PostgresStore` builds an `Option<SecretBox>` from `QUARK_ENCRYPTION_KEY` at open time. When it's `Some`, the two fields above get sealed on write and opened on read. A stored value's prefix marks its format, so a value written before encryption was turned on still reads back correctly (passthrough).

This only applies to Postgres (cloud/self-host-with-Postgres). LMDB (single-node OSS) never touches this: it stores those fields plaintext regardless, because there's no `QUARK_ENCRYPTION_KEY` handling on that backend at all.

### Wire format

A stored value is one of three shapes:

- No prefix: legacy plaintext. `open` returns it unchanged.
- `enc:v1:<base64(nonce || ciphertext)>`: the original LUC-48 format. No AAD, single key. Still opened for back-compat (every key in the keyring is tried with an empty AAD), but nothing writes this format anymore.
- `enc:v2:<keyid>:<base64(nonce || ciphertext)>`: the current format (LUC-62). `keyid` is `hex(SHA-256(key)[..4])` (8 hex chars), deterministic from the key material, and selects which keyring entry opens the value. The AEAD is bound to an **AAD** that identifies the row: `"<tenant_id>:oidc_client_secret"` or `"<tenant_id>:sheets_refresh_token"`. Copying a v2 ciphertext into another row or tenant no longer decrypts, because the AAD no longer matches (fails closed as an authentication error).

### Keyring

`SecretBox` is a keyring: one primary key from `QUARK_ENCRYPTION_KEY` (used for every new seal) plus zero or more decrypt-only old keys from `QUARK_ENCRYPTION_KEY_OLD` (comma-separated, for rotation). `open` selects a v2 key by its `keyid`; a v1 value is opened by trying every key in the ring. An invalid entry in `QUARK_ENCRYPTION_KEY_OLD` is logged and skipped (it does not turn encryption off); an invalid `QUARK_ENCRYPTION_KEY` does turn encryption off (plaintext, with a warning), same as before.

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

## Rotating the key

Rotation is online and driven by the boot backfill. The old key stays in the keyring long enough to open existing values while the backfill re-seals them under the new key.

### 1. Generate a new key and set both vars

Keep the current key as the OLD key and set the freshly generated one as the primary:

```bash
new_key="$(head -c 32 /dev/urandom | base64)"
fly secrets set \
  QUARK_ENCRYPTION_KEY="$new_key" \
  QUARK_ENCRYPTION_KEY_OLD="<the current key>" \
  -a quark-prod
```

`QUARK_ENCRYPTION_KEY_OLD` accepts several old keys separated by commas, if more than one generation is still in the database.

### 2. Boot re-seals everything

On boot with both vars set, the backfill re-seals every value that is not already `enc:v2:<new_keyid>:`, that is: plaintext, `enc:v1:`, and `enc:v2:` sealed under any old key. It opens each value with the keyring (the old key handles the rotated-out values) and re-seals it under the new primary with the correct per-row AAD. Same boot log line as before:

```
secret re-encryption backfill: <n> re-encrypted
```

It is idempotent and safe on every replica. During the window before the backfill finishes on a given node, reads still work because the old key is in the ring.

### 3. Remove the old key

Once you have confirmed the backfill ran (`n` back to 0 on a subsequent boot, meaning nothing is left under an old key), drop the OLD var:

```bash
fly secrets unset QUARK_ENCRYPTION_KEY_OLD -a quark-prod
```

After this, only the new key remains. Values are all `enc:v2:<new_keyid>:` and open under the primary alone.

## Losing the key

The key isn't recoverable from the ciphertext. If `QUARK_ENCRYPTION_KEY` (and any `QUARK_ENCRYPTION_KEY_OLD`) is lost, every sealed value in the database becomes unreadable. `SecretBox::open` fails closed (an authentication error) rather than returning garbage, so this shows up as OIDC logins failing and Sheets sync erroring out, not as silent corruption. Back up the key the same way you'd back up `QUARK_KEY`: outside the database, somewhere your secrets manager or password vault reaches.

## OSS / self-host without the key

Unset `QUARK_ENCRYPTION_KEY` (the default) and everything behaves exactly as it did before LUC-48: `client_secret` and `refresh_token` are stored plaintext, on both LMDB and Postgres. There's no forced migration, no format change, nothing to opt out of.
