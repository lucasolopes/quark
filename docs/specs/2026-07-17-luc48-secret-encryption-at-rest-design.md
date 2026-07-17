# LUC-48 — Cifrar segredos at rest (OIDC client_secret + Sheets refresh_token)

**Status:** design aprovado (usuário 2026-07-17, opt-in). Base `main @ cc1f1e2`. Cifra app-level, $0 (sem KMS).

## Objetivo

Os segredos de terceiros que o quark guarda no Postgres estão **plaintext**: o `client_secret` do OIDC por-tenant (`oidc_configs.blob`, real no modelo A onde o tenant traz o IdP) e o `refresh_token` do Google Sheets (`sheets_connection`). Cifrar ambos **at rest** com uma chave fora do banco, pra o banco deixar de ser a fronteira de confiança sozinho — relevante antes de onboarding de tenant enterprise real.

## Decisões (usuário + defaults)

1. **XChaCha20-Poly1305** (AEAD, crate `chacha20poly1305`, mesma família RustCrypto do resto). Nonce de 24 bytes (XChaCha) → nonce aleatório por operação sem risco de colisão.
2. **Chave via env `QUARK_ENCRYPTION_KEY`** (base64 de 32 bytes). Fora do banco. Sem KMS = $0.
3. **Opt-in** (decisão do usuário): env setada → escreve cifrado; env **não** setada → plaintext, comportamento atual (OSS/deploys existentes não quebram). Cloud **deve** setar. NÃO é obrigatório (um cloud sem a env não falha boot; só não cifra).
4. **Coexistência sem migração dura:** valor com prefixo `enc:v1:` → decifra; sem prefixo → plaintext legado (retorna como está). Um write posterior re-cifra. Backfill opcional no boot re-cifra os legados de uma vez.

## Arquitetura

### `src/secretbox.rs`
- `struct SecretBox { cipher: XChaCha20Poly1305 }`.
- `SecretBox::from_env() -> Option<SecretBox>`: lê `QUARK_ENCRYPTION_KEY` (base64→32 bytes); `None` se não setada ou inválida (loga e segue plaintext — opt-in).
- `seal(&self, plaintext: &str) -> String`: nonce aleatório (24B via getrandom) + encrypt; retorna `"enc:v1:" + base64(nonce ‖ ciphertext)`. String vazia → retorna vazia (não cifra o "sem segredo", ex. client público P2e).
- `open(&self, stored: &str) -> String`: se começa com `enc:v1:` → decifra (erro de decifra → erro propagado, não silencioso); senão → retorna `stored` (plaintext legado).
- Funções livres `seal_opt(&Option<SecretBox>, s)` / `open_opt(&Option<SecretBox>, s)`: sem SecretBox → passthrough; com → seal/open. Assim os call-sites não ramificam.
- Unit tests: round-trip seal→open; open de legado plaintext (sem prefixo) volta igual; open de `enc:v1:` com chave errada falha; string vazia passa; nonce diferente a cada seal (dois seals do mesmo texto diferem).

### Store (`src/store/postgres.rs`)
- `PostgresStore` ganha `secretbox: Option<SecretBox>` (construído em `open`/`open_with_replica` via `SecretBox::from_env()`).
- **OIDC:** `oidc_config_blob(cfg, &secretbox)` sela `client_secret` antes de serializar; `row_to_oidc_config(r, &secretbox)` abre ao mapear. (As duas funções livres passam a receber `&Option<SecretBox>`; os métodos `&self` passam `&self.secretbox`.)
- **Sheets:** `put_sheets_connection`/`get_sheets_connection` selam/abrem o `refresh_token` do mesmo jeito.
- Nada mais do blob muda (client_id/scopes/etc. seguem plaintext — não são segredo).

### Backfill (boot, opt-in)
- Quando `secretbox` setado: um passo no boot re-cifra os segredos legados (lê cada oidc_config/sheets_connection, se o segredo não tem prefixo `enc:v1:` re-grava selado). Idempotente (já-cifrado é pulado). Uma linha de log com a contagem. Só roda com a env setada; sem ela, no-op.

### LMDB
- LMDB (OSS single-tenant) também guarda sheets_connection? Se sim, mesmo tratamento com um `secretbox` no LmdbStore; se OSS não usa esses segredos, stub/passthrough. (Confirmar no plano.)

## Escopo
**Dentro:** `secretbox.rs` + dep; `secretbox` no PostgresStore; cifrar/decifrar `oidc client_secret` + `sheets refresh_token`; coexistência com legado; backfill de re-cifra no boot; runbook (gerar a chave, setar a env, rotação básica). Opt-in.
**Fora:** KMS/envelope de nuvem; rotação automática de chave (documentar o processo manual: setar chave nova exige re-cifrar — fora do escopo v1, anotar); cifrar outros campos (nenhum outro segredo de terceiro hoje).

## Testes
- Unit (`secretbox.rs`): round-trip, legado passthrough, chave-errada falha, vazio, nonce único.
- Store gated (PG não-superuser): com `QUARK_ENCRYPTION_KEY` setado → put_oidc_config grava `blob.client_secret` com prefixo `enc:v1:` (inspecionar a coluna crua), get devolve o plaintext; **sem** a env → grava plaintext (paridade). Mesmo pro sheets refresh_token. Legado: inserir um blob com secret plaintext, ler com a env setada → volta o plaintext (passthrough); re-gravar → vira cifrado. Backfill: legado vira cifrado, idempotente.
- Paridade OSS/sem-env: comportamento byte-a-byte de hoje.

## Riscos
1. **Perder a chave = perder os segredos** (sem a env certa, os `enc:v1:` não abrem). Mitigação: documentar bem que a chave é crítica (guardar no gerenciador de secrets do Fly, backup); opt-in evita surpresa.
2. **Trocar a chave** invalida os cifrados existentes. v1 não faz rotação automática — documentar o processo (decifrar com a antiga + re-cifrar com a nova exigiria as duas; fora do escopo, anotar como follow-up).
3. **Decifra falhando** (chave errada/dado corrompido) → erro propagado (login/sheets falha visível), não silencioso — melhor que servir lixo.
4. **`getrandom`** pro nonce — já é dep; usar o mesmo padrão do `generate_token`.
