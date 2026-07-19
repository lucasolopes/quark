# LUC-62 — Secretbox v2: AAD por-linha + rotação de chave

Data: 2026-07-19
Issue: LUC-62 (hardening v2 do LUC-48). Não-bloqueante; fora do threat model
at-rest atual. Prod será resetado, então não há dado real em risco.

## Objetivo

1. **AAD por-linha:** ligar o ciphertext ao `tenant_id`+campo, pra que copiar
   um ciphertext pra outra linha/tenant (atacante com WRITE no banco) não
   decifre.
2. **Rotação de chave:** suportar 2+ chaves (key id no prefixo) pra rotacionar
   sem downtime; backfill re-cifra pro key novo.
3. Part 3 do issue (Sheets orphan) = self-healing, **fora de escopo** (só nota).

Compatibilidade: valores `enc:v1:` e plaintext legado continuam abrindo.

## Formato de wire

- Plaintext (sem prefixo): legado, passthrough (inalterado).
- `enc:v1:<b64(nonce||ct)>`: **existente**, sem AAD, chave única. Continua
  abrindo (tentando as chaves do keyring, AAD vazio).
- `enc:v2:<keyid>:<b64(nonce||ct)>`: **novo**. `keyid` seleciona a chave; o
  AEAD é ligado ao `aad`.
  - `keyid` = hex dos primeiros 4 bytes de `SHA-256(key)` (8 chars).
    Determinístico da chave, sem config extra do operador.
  - `aad` = bytes de `format!("{tenant_id}:{field}")`, ex.
    `"7:oidc_client_secret"`, `"7:sheets_refresh_token"`.

## `src/secretbox.rs`

`SecretBox` vira um keyring:
```rust
struct KeyEntry { id: String, cipher: XChaCha20Poly1305 }
pub struct SecretBox { keys: Vec<KeyEntry>, primary: usize }
```
- `from_env`: chave primária de `QUARK_ENCRYPTION_KEY`; chaves extras
  (decrypt-only, pra rotação) de `QUARK_ENCRYPTION_KEY_OLD` (aceita várias
  separadas por vírgula). Todas base64→32 bytes; inválidas logam e são
  ignoradas (primária inválida = encryption off, como hoje). `primary` = índice
  da chave de `QUARK_ENCRYPTION_KEY`.
- `keyid(key) -> String`: hex(SHA-256(key)[..4]).
- `seal(plaintext, aad: &[u8]) -> String`: usa a chave primária; nonce aleatório
  de 24 bytes; `cipher.encrypt(nonce, Payload { msg, aad })`; devolve
  `enc:v2:<primary.id>:<b64(nonce||ct)>`. Empty input → "" (inalterado).
- `open(stored, aad: &[u8]) -> Result<String, SecretBoxError>`:
  - sem prefixo conhecido → `Ok(stored)` (plaintext legado).
  - `enc:v1:` → decodifica; tenta decifrar com CADA chave do keyring, **AAD
    vazio** (v1 não tem aad); primeiro sucesso vence; nenhuma → `DecryptFailed`.
  - `enc:v2:<keyid>:` → seleciona a chave pelo `keyid`; se nenhuma casa →
    `DecryptFailed` (ou variante `UnknownKey`); decifra com `Payload { msg, aad
    }`; AAD errado → `DecryptFailed`.
- `seal_opt`/`open_opt` ganham `aad: &[u8]`. Quando `None` (encryption off),
  passthrough (o aad é ignorado).

Adicionar `sha2` (já é dep, usado em outros módulos) pro keyid.

## Call sites (AAD)

- `src/store/postgres.rs`:
  - `oidc_config_blob(cfg, sb)` sela `cfg.client_secret` com
    `aad = format!("{}:oidc_client_secret", cfg.tenant_id.0)`; o open
    correspondente (`row_to...`) usa o mesmo aad (tem `tenant_id`).
  - `put_sheets_connection(tenant, c)` sela `c.refresh_token` com
    `aad = format!("{}:sheets_refresh_token", tenant.0)`; o get usa o mesmo.
- Definir os rótulos de campo como constantes (`const AAD_OIDC_CLIENT_SECRET`,
  `const AAD_SHEETS_REFRESH_TOKEN`) pra não divergir entre seal e open.

## Backfill (`reencrypt_legacy_secrets`, postgres.rs:2127)

Hoje re-cifra plaintext legado → v1. Estender pra re-selar QUALQUER valor que
não esteja em `enc:v2:<primary_keyid>:` — ou seja: plaintext, `enc:v1:`, e
`enc:v2:` com keyid != primária (rotação) → `enc:v2:<primary>:` com o aad certo.
Precisa abrir com o keyring (open aceita v1/v2/old) e re-selar com a primária +
aad. Idempotente; seguro em toda réplica. Atualizar `is_legacy_plaintext_secret`
→ um predicado `needs_reseal(value, primary_keyid)`.

## Docs

`docs/RUNBOOK-secret-encryption.md`: procedimento de rotação —
1. gerar chave nova; setar `QUARK_ENCRYPTION_KEY=<nova>` e
   `QUARK_ENCRYPTION_KEY_OLD=<antiga>`; 2. bootar (o backfill re-cifra pra nova);
3. depois de confirmar, remover `QUARK_ENCRYPTION_KEY_OLD`. Documentar o formato
`enc:v2:<keyid>:` e o AAD (bind à linha). Sem em-dash.

## Testes (TDD, cripto — abrangentes)

Em `src/secretbox.rs` mod tests:
- v2 round-trip com aad (seal→open mesmo aad = plaintext).
- open v2 com AAD DIFERENTE → `DecryptFailed` (o bind funciona).
- open v2 cujo keyid não está no keyring → erro (não decifra).
- **rotação:** valor selado com KEY_B (adicionada como old no keyring cuja
  primária é KEY_A) abre; seal novo usa o keyid de KEY_A.
- **v1 back-compat:** um valor `enc:v1:` (selado no formato v1) ainda abre com o
  keyring, aad ignorado.
- wrong-key (keyring sem a chave) falha; tamper falha; empty passthrough;
  plaintext passthrough; keyid estável (mesma chave → mesmo id).
- `needs_reseal`: true pra plaintext/v1/v2-old, false pra v2-primária.
Manter/atualizar os testes existentes (assinatura de open/seal mudou pra ter
aad — os testes atuais passam `b""`).

## Critérios de aceite

- [ ] `enc:v2:<keyid>:` com AAD `tenant:field`; copiar ciphertext pra outra
      linha/tenant não decifra (AAD mismatch).
- [ ] Keyring com primária + old(s); open seleciona por keyid; v1/plaintext
      ainda abrem.
- [ ] Backfill re-cifra plaintext/v1/old → v2-primária com aad.
- [ ] Runbook de rotação atualizado.
- [ ] Testes cripto acima verdes; suíte + clippy + fmt verdes.

## Nota de review
Mudança cripto — o review deve ser adversarial: conferir que o AAD é
verificado de fato (não só anexado), que o v1 não aceita aad por engano, que a
seleção de keyid não permite downgrade, e que nenhum caminho vaza plaintext em
erro.
