# LUC-46 — Accept de convite: claim + grant numa transação

Data: 2026-07-19
Issue: LUC-46 (P2c fast-follow). Prioridade baixa; não é buraco de segurança
(disponibilidade em falha rara de write).

## Diagnóstico

`admin_invites_accept` (`src/api.rs:3227`) hoje faz, em dois passos separados no
pool pelado:
1. `mark_invite_accepted(inv.id, user_id, now)` — claim atômico
   (`UPDATE invites SET accepted_at=... WHERE id=? AND accepted_at IS NULL` →
   bool), garante single-use (fecha a TOCTOU).
2. `put_membership(&Membership)` — concede o acesso.

Edge: se (2) falhar DEPOIS de (1) retornar `true`, o convite fica consumido sem
membership; o usuário precisa de um convite novo.

## Design

Novo método no trait `Store`:
```rust
/// Claims the invite (single-use) AND grants the membership in ONE
/// transaction: if the membership write fails, the claim is rolled back so
/// the invite stays pending. Returns Ok(true) when claimed+granted, Ok(false)
/// when the invite was already consumed (lost the race / not pending).
async fn accept_invite_tx(&self, invite_id: i64, membership: &Membership, now: u64)
    -> Result<bool, StoreError>;
```
(usar o tipo do `invite_id` que `mark_invite_accepted` já usa; o `tenant` vem de
`membership.tenant_id`.)

- **Postgres** (`begin_tenant_tx(membership.tenant_id)`, mesmo padrão de
  `put_link_tx`/os outros `_tx`): dentro de UMA transação —
  `UPDATE invites SET accepted_at=$now, accepted_by=$user WHERE id=$id AND
  accepted_at IS NULL` (RETURNING/rows_affected); se 0 → commit e `Ok(false)`
  (não claimou); se 1 → `INSERT ... memberships ...` (a mesma query do
  `put_membership`); COMMIT. Qualquer erro no INSERT → a tx dá rollback
  (o `?` propaga e o `tx` é dropado sem commit) → o claim é revertido → `Err`.
  Preservar o gate PG não-superuser (RLS/`app.tenant_id` via `begin_tenant_tx`).
- **LMDB** (`src/store/lmdb.rs`): uma única `write_txn` fazendo o claim
  (checar `accepted_at IS NULL`, setar) e o put da membership; commit no fim.
  Single-node, mas manter atômico (mesmo txn). Retorna `Ok(false)` se já
  aceito.
- Stubs de mock (`domain_router.rs`, `webhooks/delivery.rs`) com
  `unimplemented!()` ou delegando, conforme o padrão dos outros métodos.

### Handler (`src/api.rs:3290-3305`)
Trocar os dois passos (`mark_invite_accepted` + `put_membership`) por uma
chamada a `accept_invite_tx(inv.id, &membership, now())`:
- `Ok(true)` → segue (sucesso, como hoje).
- `Ok(false)` → `NOT_FOUND` (perdeu a corrida / já consumido — igual ao
  `Ok(false)` do `mark_invite_accepted` hoje).
- `Err(_)` → `SERVICE_UNAVAILABLE`. Como é atômico, um erro aqui NÃO consumiu o
  convite.
Manter a pré-checagem `get_membership` → CONFLICT e todo o resto do handler.
Os métodos antigos `mark_invite_accepted`/`put_membership` podem continuar
existindo (usados noutros lugares? conferir); o handler passa a usar o `_tx`.

## Testes
- Integração (`tests/*invite*_it.rs` ou onde os testes de convite vivem, gated
  em `QUARK_TEST_DATABASE_URL` pro Postgres): 
  - accept feliz → `Ok(true)`, membership existe, invite marcado aceito.
  - **atomicidade:** simular falha no grant (ex. membership que viola uma
    constraint, ou um segundo accept concorrente) e verificar que o invite NÃO
    fica consumido sem membership. Se não der pra injetar falha de grant
    facilmente, ao menos um teste de que dois accepts concorrentes resultam em
    exatamente 1 membership e 1 claim (single-use preservado).
  - o teste de single-use/claim atual continua verde.
- LMDB: teste unit/integração do caminho atômico (claim+grant juntos; segundo
  accept → false).

## Critérios de aceite
- [ ] Falha no grant pós-claim NÃO consome o convite (atômico, rollback).
- [ ] Single-use sob concorrência preservado.
- [ ] Gate PG não-superuser mantido (`begin_tenant_tx`).
- [ ] Suíte + clippy + fmt verdes (testes PG gated podem não rodar sem DB, mas
      compilam).
