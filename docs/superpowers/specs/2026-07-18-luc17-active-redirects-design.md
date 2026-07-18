# LUC-17 — Visualização dos redirects ativos

Data: 2026-07-18
Issue: LUC-17

## Definição

Um link é **ativo** quando:
- `expiry` é `NULL` OU `expiry > now` (não expirado por data), E
- `max_visits` é `NULL` OU `visits < max_visits` (não esgotado por limite de visitas).

`now` = `crate::now()` (epoch secs UTC).

## Investigação (fatos)

- `admin_links_list` (`src/api.rs:3679`) já tem filtros `q`/`tag`/`folder`/`health=broken`. O `broken` usa um método dedicado (`list_broken_link_ids`) porque o conjunto é pequeno. "Ativo" é diferente: o conjunto é grande (a maioria dos links), então NÃO cabe um `list_active_ids`; o filtro tem que entrar na própria query de listagem/paginação.
- **Postgres**: a tabela `links` tem as colunas `expiry`, `max_visits` e `visits` (`src/store/postgres.rs:633-634`). O filtro "ativo" é uma cláusula WHERE numa query só, sem N+1.
- **LMDB**: as visitas ficam num store separado (`bump_visits`/`visits`, lmdb.rs:505+); o `expiry` está no `Record`. O `list_links` do LMDB itera Records; para "ativo" filtra por `expiry` (in-record) e, para Records com `max_visits`, faz o lookup de `visits` (barato: embedded, single-node). Só faz o lookup quando `max_visits.is_some()`.
- O filtro `broken` ignora `q` (search). "Ativo" deve compor com `q`/`tag`/`folder` (é um predicado adicional), então entra tanto no `list_links` quanto no `search_links`.

## Design

### Store trait (`src/store/mod.rs`)
Adicionar um parâmetro `active_only: bool` a `list_links` e `search_links`. Todos os call sites existentes passam `false` (sheets, health, api sem o filtro, domain_router mock, testes lmdb). `now` NÃO entra na assinatura: cada backend usa `crate::now()` internamente para a comparação de `expiry`.

- **Postgres** (`list_links` L1202, `search_links`): quando `active_only`, adicionar ao WHERE:
  `AND (expiry IS NULL OR expiry > <now>) AND (max_visits IS NULL OR visits < max_visits)`.
  Manter a paginação por id e o `limit` como estão (o filtro só estreita o conjunto; a paginação por keyset continua correta).
- **LMDB** (`list_links` L366, `search_links` se existir): ao montar a página, pular Records não-ativos (expiry passado; ou `max_visits` set e `visits(tenant,id) >= max_visits`). Como a paginação do LMDB é por keyset de id com `limit`, aplicar o predicado ANTES de contar o `limit` (a página final tem só ativos e o cursor aponta pro último id incluído).

### Handler (`src/api.rs`)
- `ListParams` ganha um campo (ex. `status: Option<String>` com valor `"active"`, seguindo o padrão do `health=broken`; OU um bool `active`). Preferir `status=active` pra simetria com `health=broken`.
- `admin_links_list`: computa `active_only = p.status.as_deref() == Some("active")` e passa pro `list_links`/`search_links`. Compõe com tag/folder/q normalmente. (O ramo `broken_only` é separado e não precisa do active; se ambos vierem, broken vence, como hoje.)

### Frontend (`web/src/routes/Links.tsx` + `web/src/components/LinkTable.tsx` + `web/src/lib/queries.ts` + i18n)
- Um toggle/segmented control "Ativos" vs "Todos" no topo da lista (perto dos filtros existentes de tag/folder/broken).
- Quando "Ativos", a query manda `status=active`. O hook `useLinks` (ou equivalente) passa o param. Reusar o padrão do filtro `broken`/`health` já existente no front (se houver).
- i18n EN + pt-BR pros labels do toggle. Sem em-dash.

## Testes (TDD)

- **Rust**: teste de store (LMDB, via `open_backends`) — criar links: (a) ativo simples, (b) expirado (`expiry` no passado), (c) esgotado (`max_visits=1`, `bump_visits` até atingir), (d) ativo com `max_visits` alto. `list_links(active_only=true)` retorna só (a) e (d). Um teste de handler/integração em `tests/` se o padrão existir (ver `api_it.rs`).
- **Web**: teste do toggle em `Links.test.tsx` (se existir) — alternar pra "Ativos" dispara a query com `status=active`.

## Critérios de aceite

- [ ] "ativo" = (sem expiry ou futuro) E (sem max_visits ou visits < max_visits).
- [ ] Filtro server-side em `list_links`/`search_links` (sem carregar tudo pro front).
- [ ] Postgres: cláusula WHERE única (sem N+1). LMDB: lookup de visits só quando `max_visits` set.
- [ ] Toggle no painel; compõe com tag/folder/q.
- [ ] OSS single-tenant inalterado no comportamento default (sem o filtro = lista tudo, como hoje).
- [ ] Suíte Rust + web tests + clippy + fmt + tsc verdes.
