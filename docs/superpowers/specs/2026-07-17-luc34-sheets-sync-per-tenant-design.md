# LUC-34 — Google Sheets sync por tenant

Data: 2026-07-17
Issue: LUC-34 (Google Sheets como extensão OAuth por tenant)
Depende de: LUC-7 (carry-over do OAuth carregar tenant no state — JÁ FEITO)

## Diagnóstico

A parte de OAuth já está por tenant: `sheets_connect` grava o tenant do
principal no slot `verifier` do state assinado, e `sheets_callback` o lê de
volta e persiste a conexão sob aquele tenant. Os handlers `sheets_status`,
`sheets_sync` e `sheets_disconnect` já operam sob `p.tenant`.

O gap está no sync em si, `src/sheets/mod.rs::sync`, que hardcoda
`DEFAULT_TENANT` ao ler o catálogo:

- `store.list_links(DEFAULT_TENANT, ...)` (mod.rs:252)
- `store.visits(DEFAULT_TENANT, id)` (mod.rs:272)

`sync` não recebe tenant. Consequências:

1. **Bug de correção no on-demand:** o handler `sheets_sync` (api.rs:3533)
   carrega a `conn` do `p.tenant` mas chama `sync`, que lê os links do
   `DEFAULT_TENANT`. Um tenant não-default que aperta "Sync now" escreve os
   links do tenant 0 na própria planilha.
2. **Worker agendado só sincroniza o tenant 0:** `main.rs` faz
   `get_sheets_connection(DEFAULT_TENANT)` e `put_sheets_connection(DEFAULT_TENANT)`
   num tick só; tenants cloud nunca sincronizam.

## Escopo

1. `sheets::sync` ganha um parâmetro `tenant: TenantId` e o usa em `list_links`
   e `visits`.
2. Handler on-demand `sheets_sync` (api.rs) passa `p.tenant` para `sync`.
3. Worker agendado (main.rs) itera `list_tenants()` e, para cada tenant com uma
   `SheetsConnection`, faz refresh + sync + persist sob aquele tenant. Segue o
   padrão do LUC-36 (iterar tenants, sem novo método no trait `Store`). A lease
   `sheets_lease` continua global: uma aquisição por tick cobre a varredura
   inteira.
4. Erro de um tenant (refresh ou sync) grava `SyncStatus::Error` na conexão
   daquele tenant e continua a varredura; não aborta os demais.

Fora de escopo:
- **Base URL por tenant nas short URLs da planilha.** O sync agendado usa o
  `QUARK_PUBLIC_HOST` global para montar as short URLs, igual hoje. Um tenant
  com domínio próprio teria short URLs no host global. Resolver o domínio
  primário por tenant é follow-up (não está no AC do LUC-34).
- Outros conectores OAuth nativos.

## Design

### `sheets::sync` (assinatura)

```rust
pub async fn sync(
    store: &Arc<dyn Store>,
    api: &dyn client::SheetsApi,
    key: u64,
    base_url: &str,
    conn: &mut SheetsConnection,
    access_token: &str,
    now: u64,
    tenant: TenantId,   // NOVO, por último para minimizar ruído nos call sites
) -> Result<(), String>
```

Trocar `DEFAULT_TENANT` por `tenant` em `list_links` e `visits`.

### Worker agendado (main.rs)

Em vez de um `get_sheets_connection(DEFAULT_TENANT)`, dentro do tick (após
adquirir a lease):

```
tenants = store.list_tenants()   // erro: loga e pula o tick
para cada t em tenants:
    conn = get_sheets_connection(t.id)   // None => pula t
    outcome = refresh_access_token(...) then sync(..., &mut conn, ..., t.id)
    on error: conn.last_status = Error(e); log
    put_sheets_connection(t.id, &conn)   // log em erro
```

OSS/single-tenant: `list_tenants` devolve só o default → um tenant no loop,
comportamento idêntico ao de hoje.

## Testes (TDD)

Não existe teste que exercite `sheets::sync` com mock. Criar
`tests/sheets_sync_it.rs`:

- Mock `SheetsApi` que registra os `rows` passados a `update_values` (e
  devolve um id fixo em `create_spreadsheet`).
- **Teste principal (isolamento por tenant):** dois tenants (0 e 1), cada um
  com um link distinto no store; chamar `sync(..., tenant=1)`; assertar que os
  rows escritos contêm o `destination` do link do tenant 1 e NÃO o do tenant 0.
- Prova simultaneamente que sync != DEFAULT_TENANT lê o catálogo certo.

## Critérios de aceite

- [ ] `sheets::sync` recebe e respeita `tenant` (`list_links`/`visits`).
- [ ] On-demand `sheets_sync` passa `p.tenant` (sincroniza os links do tenant
      certo).
- [ ] Worker agendado itera `list_tenants()` e sincroniza cada tenant com
      conexão; erro de um não aborta os demais.
- [ ] OSS/single-tenant inalterado.
- [ ] Teste de isolamento por tenant verde; suíte completa verde.
- [ ] docs/SHEETS.md + .PT_BR.md refletem o escopo por tenant (se afirmarem
      single-tenant/uma conexão global).
