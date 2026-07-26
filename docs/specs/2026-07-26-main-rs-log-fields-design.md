# LUC-142 — Campos nativos de log no `main.rs` (design)

**Data:** 2026-07-26
**Estado:** aprovado no brainstorming
**Issue:** LUC-142 (Medium)

## Objetivo

Oito sítios em `src/main.rs` montam JSON à mão dentro da macro de log:

```rust
tracing::info!(
    "{}",
    serde_json::json!({ "analytics_purge_deleted": n, "cutoff_ts": cutoff })
);
```

Com `QUARK_LOG_FORMAT=json` isso produz um JSON serializado **dentro** do campo
`message` do JSON do log:

```json
{"level":"INFO","fields":{"message":"{\"analytics_purge_deleted\":0,\"cutoff_ts\":1753510102}"}}
```

Contra o que os módulos convertidos produzem:

```json
{"level":"INFO","fields":{"message":"sheets sync completed","tenant":1}}
```

O ponto de emitir JSON é o pipeline conseguir filtrar e agregar por campo. Com
o JSON aninhado numa string escapada, `analytics_purge_deleted` e `cutoff_ts`
não são campos, são texto. Não dá para alertar nem montar gráfico em cima
deles sem desserializar a `message` de novo.

## Contexto

O LUC-130 converteu os módulos de biblioteca e deixou o `main.rs` para trás. O
resultado é que as duas formas convivem no mesmo arquivo, e em
`spawn_sheets_sync` elas estão a cinco linhas de distância:

```rust
tracing::warn!(error = %e, tenant = t.id.0, "sheets sync failed");            // :763
tracing::info!("{}", serde_json::json!({ "sheets_sync_persist_error": ... })); // :768
```

Isso só ficou visível quando o `QUARK_LOG_FORMAT=json` foi ligado em
`quark-prod` depois da v0.4.0.

## Os oito sítios, e o nível de cada um

A convenção do repo está em
`.claude/skills/quark-rust/references/errors-and-observability.md`: `error!`
para o que precisa de atenção, `warn!` para **todo** caminho fail-open, `info!`
para boot e lifecycle.

| Linha | O que é | Hoje | Correto |
|---|---|---|---|
| 211 | erro do backfill de subdomínio | `info!` | `warn!` |
| 217 | erro ao resolver o host no backfill | `info!` | `warn!` |
| 727 | `list_tenants` falhou no sheets sync | `info!` | `warn!` |
| 768 | persistir a conexão do sheets falhou | `info!` | `warn!` |
| 795 | GC de sessão falhou | `info!` | `warn!` |
| 825 | retenção configurada (boot) | `info!` | `info!` |
| 839 | purga concluída (sucesso) | `info!` | `info!` |
| 843 | erro da purga | `info!` | `warn!` |

**Seis dos oito logam erro como `info!`.** Não é detalhe de estilo: um
alerta montado em cima de `level=ERROR` ou `level=WARN` nunca dispararia para
nenhuma dessas falhas. O comentário do próprio código na purga
(`main.rs:829-830`) diz "fail-open: a purge error is only logged and never
blocks serving", e a linha logo abaixo usa `info!`.

Nenhum dos oito precisa de `error!`: são todos degradações que o processo
absorve e segue, ou sucesso.

## Decisões

1. **Campos nativos, com a sintaxe da convenção**: `%value` para `Display`,
   `field = value` para primitivos. A mensagem é frase humana curta; o dado vai
   nos campos.
2. **Níveis corrigidos** conforme a tabela. Os dois casos de boot e sucesso
   continuam `info!`.
3. **Uma invariante que impede a volta.** Hoje nada impede: não há lint no
   `Cargo.toml`, nada no `clippy.toml`, e o CI não tem checagem textual. Foi
   exatamente por isso que o LUC-130 deixou o `main.rs` para trás sem ninguém
   notar. Entra um teste que varre `src/` e falha se achar `json!` dentro de
   macro de log.

   Ao contrário da invariante equivalente criada na LUC-140, esta asserta
   **zero ocorrências**, não uma contagem. Não precisa de manutenção quando
   alguém adiciona um log correto, e só quebra quando alguém reintroduz o
   padrão errado, que é o comportamento desejado.

## Nomes dos campos

As chaves JSON de hoje carregam o contexto no próprio nome
(`sheets_sync_persist_error`, `analytics_purge_deleted`) porque não havia
mensagem. Com mensagem e campos separados, o contexto vai para a mensagem e o
campo fica com o nome curto, como os módulos já convertidos fazem:

| Antes | Depois |
|---|---|
| `{"tenant_subdomain_backfill_error": e, "tenant_id": id}` | `warn!(error = %e, tenant = id, "tenant subdomain backfill failed")` |
| `{"sheets_sync_list_tenants_error": e}` | `warn!(error = %e, "sheets sync could not list tenants")` |
| `{"sheets_sync_persist_error": e, "tenant": id}` | `warn!(error = %e, tenant = id, "sheets sync persist failed")` |
| `{"session_gc_error": e}` | `warn!(error = %e, "session gc failed")` |
| `{"analytics_retention_secs": r}` | `info!(retention_secs = r, "analytics retention configured")` |
| `{"analytics_purge_deleted": n, "cutoff_ts": c}` | `info!(deleted = n, cutoff_ts = c, "analytics purge completed")` |
| `{"analytics_purge_error": e}` | `warn!(error = %e, "analytics purge failed")` |

Isso é mudança de formato de log. Não há consumidor automatizado hoje (não
existe alerta nem dashboard montado sobre essas chaves, e o formato JSON só foi
ligado em 2026-07-26), então não há nada para migrar.

## Fora de escopo

Uma lint de clippy que proíba o padrão em geral. `clippy` não tem lint pronta
para isso e escrever uma exigiria `dylint`, que é dependência nova de
ferramenta. A invariante de teste cobre o mesmo risco com muito menos.

## Restrições do projeto

Nada toca o hot path do redirect. `codec.rs` e `permute.rs` intocados. O gate é
`cargo fmt`, `cargo clippy --all-targets -D warnings` e os testes.
