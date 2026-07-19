# LUC-50 — Invalidação cross-replica do HostRouter (`host:<name>`)

Data: 2026-07-19
Issue: LUC-50 (P3 follow-up)

## Diagnóstico (investigação)

Muita coisa já está no lugar:
- `HostRouter` (`src/domain_router.rs`) já tem o campo
  `invalidator: Option<Arc<Invalidator>>` e um método `invalidate(host)` que
  hoje só dropa o L1 local (não publica).
- Os sites de mutação de domínio já chamam `st.host_router.invalidate(host)`:
  seed de subdomínio (`api.rs:2356`), add/verify (`:2592`), remove (`:2621`).
- O `Invalidator` já é construído no `main.rs:190` e passado pro `Cache`; o
  `HostRouter::new` (`main.rs:346`) hoje recebe `None`.
- O canal pub/sub (`src/invalidate.rs`) só parseia `link:<id>` →
  `cache.invalidate_local(id)`.

Faltam 4 peças: publicar `host:<name>`, consumir `host:<name>`, um
`invalidate_local` no HostRouter (pro subscriber, sem re-publish), e passar o
`Invalidator` real no main.

## Design

### `src/domain_router.rs`
- Extrair `invalidate_local(&self, host)`: normaliza a `key` (mesma lógica de
  hoje) e dropa só o L1 (`self.cache.invalidate(&key)`). Sem publish.
- `invalidate(&self, host)` = `self.invalidate_local(host)` + se
  `self.invalidator` for `Some`, `inv.publish(&format!("host:{host}")).await`.
  (Best-effort/fail-open, como o cache.) Os call sites existentes não mudam.

### `src/invalidate.rs`
- `Invalidation::Host(String)` no enum.
- `parse_message`: também aceita `host:<name>` — `name` = o resto após
  `host:`, **não-vazio** (senão `None`). (Cuidado: o `name` pode conter `:`?
  Um host DNS não contém `:`; mas pra robustez, tratar todo o resto como o
  nome, sem re-split.)
- `run_once`: no `Some(Invalidation::Host(name))` →
  `state.host_router.invalidate_local(&name).await` (LOCAL-only, nunca
  re-publica — evita loop cross-node, igual ao `link:`).

### `src/main.rs`
- `HostRouter::new(store, public_host, invalidator.clone())` no lugar do `None`
  (linha 346).

## Testes

- Unit (`invalidate.rs`): `parse_message` aceita `host:go.acme.com` →
  `Host("go.acme.com".into())`; rejeita `host:` (vazio); `link:` continua
  funcionando; garbage → `None`.
- Unit (`domain_router.rs`): com um `Invalidator { conn: None }` (no-op),
  `invalidate` dropa o L1 (o teste existente `invalidate_drops_cache_entry...`
  continua). Adicionar um teste de que `invalidate_local` também dropa o L1.
  (A publicação de fato é best-effort e testada de ponta a ponta no
  integration gated.)
- Integração (`tests/pubsub_invalidation_it.rs`, gated em `QUARK_TEST_VALKEY_URL`):
  publicar `host:<name>` num nó e ver o L1 do HostRouter de outro nó ser
  dropado (espelha o teste de `link:` existente, se houver; senão, adicionar).

## Critérios de aceite
- [ ] `host:<name>` publicado no add/remove/verify de domínio e no seed de
      subdomínio (via `invalidate` agora publicar).
- [ ] Subscriber consome `host:<name>` → `host_router.invalidate_local`.
- [ ] `HostRouter` recebe o `Invalidator` real no `main.rs`.
- [ ] Sem loop cross-node (subscriber usa `invalidate_local`, não `invalidate`).
- [ ] Single-node (sem Valkey) inalterado (`invalidator=None` → publish no-op).
- [ ] Testes acima verdes; suíte + clippy + fmt verdes.
