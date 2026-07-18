# LUC-37 — Operações em massa no painel

Data: 2026-07-18
Issue: LUC-37

## Escopo

Selecionar vários links na tabela do painel e aplicar uma operação de uma vez:
`delete`, `add_tag`, `remove_tag`, `set_folder`. Novo endpoint
`POST /admin/links/bulk` (admin-side, sem impacto no hot path). Resposta
parcial por item (como o importer). Fora de escopo: criação em massa (já existe
via import) e edição de regras/variants em lote.

## Investigação (padrão de mutação por-link, `src/api.rs`)

Handlers `admin_link_delete` (3878) e `admin_link_patch` (3924):
- `resolve_for_admin(&st, p.tenant, &code)` → `(id, alias)` (ou 404).
- `get_link(p.tenant, id)` → `rec`.
- Mutação: edita `rec` (tags via `normalize_tags`, folder, etc.).
- Webhook lifecycle: monta `WebhookEvent` (`LinkUpdated`/`LinkDeleted`,
  `webhook_event_payload(...)`, `tenant_id: p.tenant`), `rows =
  lifecycle_deliveries(p.tenant, &ev)`, e persiste com `put_link_tx`/
  `delete_link_tx(p.tenant, id, &rows)`.
- `st.cache.invalidate(id)` + `st.webhooks.emit_if_in_memory(ev)`.

O bulk REUSA exatamente essas primitivas por item.

## Design

### Endpoint `POST /admin/links/bulk` (`src/api.rs`)
- Guard: `admin_guard(Scope::LinksWrite)`; tenant do principal.
- Body JSON:
  ```json
  { "codes": ["abc","def"], "op": "add_tag", "value": "promo" }
  ```
  - `op`: `"delete" | "add_tag" | "remove_tag" | "set_folder"`.
  - `value`: obrigatório para `add_tag`/`remove_tag` (a tag) e `set_folder`
    (o folder; string vazia ou null = remover da pasta, `rec.folder = None`).
    Ignorado para `delete`.
  - Limite de itens por request (ex. `MAX_BULK = 500`, alinhado ao page limit)
    para evitar request gigante; acima disso, 400.
- Para cada `code`: resolve → get_link → aplica a op → persiste com o mesmo
  caminho tx + evento lifecycle + `cache.invalidate` + `emit_if_in_memory` do
  handler individual. Erros por item (não encontrado, store) NÃO abortam os
  demais.
  - `add_tag`: `rec.tags` recebe a tag (via `normalize_tags` no conjunto todo,
    idempotente se já existe). `LinkUpdated`.
  - `remove_tag`: remove a tag de `rec.tags`. `LinkUpdated`.
  - `set_folder`: `rec.folder = Some(value)` ou `None` se vazio. `LinkUpdated`.
  - `delete`: mesmo caminho do `admin_link_delete` (delete_link_tx + alias +
    invalidate + `LinkDeleted`).
- Resposta 200 com relatório por item:
  ```json
  { "ok": 2, "failed": 1, "results": [ {"code":"abc","ok":true}, {"code":"xyz","ok":false,"error":"not found"} ] }
  ```
  Espírito do `src/import.rs` (relatório parcial).

### Store
Nenhum método novo no trait — reusa `get_link`/`put_link_tx`/`delete_link_tx`/
`delete_alias`/`resolve` já existentes. (Uma única transação por item, como o
handler individual; não precisa de tx multi-item.)

### Frontend (`web/src/components/LinkTable.tsx` + `Links.tsx` + `queries.ts`/`api.ts` + i18n)
- Checkbox por linha + checkbox "selecionar todos (da página)" no header.
- Estado de seleção (set de codes) em `Links.tsx` (ou LinkTable via
  callback/props).
- Barra de ações em massa quando há seleção: Deletar (com `AlertDialog` de
  confirmação, contagem), Adicionar tag (input), Remover tag (input), Mover
  pra pasta (input). Cada uma chama `api.bulkLinks(codes, op, value)` via um
  hook `useBulkLinks` que invalida a query de links no sucesso.
- Toast com o resumo (`ok`/`failed`); se `failed > 0`, detalhar quantos.
- i18n EN + pt-BR, sem em-dash.

## Testes

- **Rust** (`tests/api_it.rs`, seguindo o padrão dos testes de admin lá):
  criar N links, `POST /admin/links/bulk` com `add_tag` → todos ganham a tag;
  `delete` com uma mistura de codes válidos + um inexistente → relatório
  `ok`/`failed` correto e os válidos somem; auth (sem token → 401);
  `set_folder` move; `remove_tag` remove.
- **Web**: teste de seleção múltipla + uma ação em massa em `LinkTable`/`Links`
  (mockar `bulkLinks`), asserta que chama com os codes/op certos.

## Critérios de aceite

- [ ] `POST /admin/links/bulk` com `delete`/`add_tag`/`remove_tag`/`set_folder`,
      reusando as primitivas por-link (evento lifecycle + cache invalidate).
- [ ] Relatório parcial por item.
- [ ] Multi-seleção + barra de ações na `LinkTable`.
- [ ] Sem tocar no hot path do redirect.
- [ ] Suíte Rust + web + clippy + fmt + tsc verdes.
