# LUC-142 — Campos nativos de log no `main.rs` (plano)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Os oito sítios de `src/main.rs` que montam JSON à mão dentro da macro de log passam a emitir campos nativos do `tracing`, com os níveis corrigidos, e uma invariante impede que o padrão volte.

**Architecture:** Conversão mecânica de `tracing::info!("{}", serde_json::json!({...}))` para `tracing::warn!(error = %e, campo = valor, "mensagem")`, seguindo o que os módulos de biblioteca já fazem. Mais um teste que varre o fonte de `src/` e falha se achar `json!` dentro de macro de log.

**Tech Stack:** Rust, `tracing`, `tracing-subscriber`.

**Spec:** `docs/specs/2026-07-26-main-rs-log-fields-design.md`

## Global Constraints

- Nada toca o hot path do redirect. `src/codec.rs` e `src/permute.rs` intocáveis.
- Nenhuma crate nova.
- Comentários e chaves de log em inglês.
- Convenção de níveis (`.claude/skills/quark-rust/references/errors-and-observability.md`): `error!` para o que precisa de atenção, `warn!` para **todo** caminho fail-open, `info!` para boot e lifecycle. Sintaxe: `%value` para `Display`, `?value` para `Debug`, `field = value` para primitivos. Mensagem é frase humana curta; o dado vai nos campos.
- Gate: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --lib`. Sempre `-j1` / `CARGO_BUILD_JOBS=1`.

---

### Task 1: A invariante que impede a volta

Vem primeiro de propósito: escrita antes da conversão, ela **falha** contra o código atual, e é a prova de que os oito sítios existem. Depois da Task 2 ela vira a guarda.

**Files:**
- Create: `tests/log_convention_it.rs`

**Interfaces:**
- Produces: nada consumido por outras tasks.

- [ ] **Step 1: Escrever o teste**

```rust
//! Invariante de convenção de log: nenhuma macro de `tracing` em `src/` pode
//! montar a mensagem com `serde_json::json!`.
//!
//! O padrão legado produz um JSON serializado dentro do campo `message` do
//! JSON do log, o que transforma os dados em texto e mata a agregação por
//! campo, que é o motivo de emitir JSON. O LUC-130 converteu os módulos de
//! biblioteca e deixou o `main.rs` para trás sem que ninguém notasse, porque
//! não havia nada checando. Isto é esse algo.

/// Arquivos de `src/` varridos. `include_str!` amarra a checagem ao conteúdo
/// real compilado, então um arquivo novo precisa ser adicionado aqui de
/// propósito, e não passa despercebido por um glob que ninguém leu.
const SOURCES: &[(&str, &str)] = &[
    ("src/main.rs", include_str!("../src/main.rs")),
    ("src/lib.rs", include_str!("../src/lib.rs")),
];

#[test]
fn no_log_macro_builds_its_message_with_serde_json() {
    let macros = ["info!", "warn!", "error!", "debug!", "trace!"];
    let mut offenders = Vec::new();

    for (path, src) in SOURCES {
        for (idx, line) in src.lines().enumerate() {
            let Some(macro_col) = macros.iter().find_map(|m| line.find(m)) else {
                continue;
            };
            // A macro e o `json!` podem estar na mesma linha ou em linhas
            // seguidas (o rustfmt quebra a chamada). Olhar as duas linhas
            // seguintes cobre as duas formas.
            let window: String = src
                .lines()
                .skip(idx)
                .take(3)
                .collect::<Vec<_>>()
                .join(" ");
            let _ = macro_col;
            if window.contains("json!") {
                offenders.push(format!("{path}:{}: {}", idx + 1, line.trim()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "log macros must use tracing fields, not a hand-built json! message.\n\
         The nested json ends up escaped inside `message` and stops being \
         queryable.\n{}",
        offenders.join("\n")
    );
}
```

- [ ] **Step 2: Rodar e confirmar que FALHA, listando os oito**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
CARGO_BUILD_JOBS=1 cargo test -j1 --test log_convention_it 2>&1 | tail -25
```

Esperado: FAIL, com os oito sítios de `src/main.rs` listados por linha. **Confira que são oito.** Se o teste achar menos, ele tem falso negativo e precisa ser consertado antes da Task 2; se achar mais, o levantamento do spec estava incompleto e você deve reportar quais são os extras.

- [ ] **Step 3: Commit do teste vermelho**

Não. Não commite um teste que falha. Siga para a Task 2 e commite as duas juntas, ou marque o teste com `#[ignore]` temporariamente **apenas se** precisar interromper o trabalho no meio. O commit final tem a suíte verde.

---

### Task 2: Converter os oito sítios

**Files:**
- Modify: `src/main.rs` (linhas 211, 217, 727, 768, 795, 825, 839, 843)

**Interfaces:**
- Consumes: a invariante da Task 1.

- [ ] **Step 1: Converter, um a um**

As linhas exatas e a forma alvo. Note que os números de linha se deslocam conforme você edita; localize por conteúdo, não por número.

**`main.rs:211` e `:217`, backfill de subdomínio de tenant.** Os dois têm hoje a mesma chave (`tenant_subdomain_backfill_error`) para causas diferentes: um é `seed_tenant_subdomain` falhando, o outro é `get_domain_by_host` falhando. Dê mensagens distintas, porque a chave única de hoje os torna indistinguíveis no log.

```rust
Err(e) => tracing::warn!(
    error = %e,
    tenant = t.id.0,
    "tenant subdomain backfill failed"
),
```
```rust
Err(e) => tracing::warn!(
    error = %e,
    tenant = t.id.0,
    "tenant subdomain backfill could not resolve the host"
),
```

**`main.rs:727`, `list_tenants` no sheets sync:**
```rust
Err(e) => {
    tracing::warn!(error = %e, "sheets sync could not list tenants");
    continue;
}
```

**`main.rs:768`, persistir a conexão do sheets.** O vizinho cinco linhas acima já está na forma certa (`tracing::warn!(error = %e, tenant = t.id.0, "sheets sync failed")`); use `tenant` com o mesmo nome, não `tenant_id`, para os dois agregarem juntos:
```rust
if let Err(e) = store.put_sheets_connection(t.id, &conn).await {
    tracing::warn!(error = %e, tenant = t.id.0, "sheets sync persist failed");
}
```

**`main.rs:795`, GC de sessão:**
```rust
if let Err(e) = store.gc_sessions(quark::now()).await {
    tracing::warn!(error = %e, "session gc failed");
}
```

**`main.rs:825`, retenção configurada. Segue `info!`, é boot:**
```rust
tracing::info!(retention_secs = retention, "analytics retention configured");
```

**`main.rs:839` e `:843`, a purga:**
```rust
match store.purge_click_events_before(cutoff).await {
    Ok(n) => tracing::info!(
        deleted = n,
        cutoff_ts = cutoff,
        "analytics purge completed"
    ),
    Err(e) => tracing::warn!(error = %e, "analytics purge failed"),
}
```

- [ ] **Step 2: Rodar a invariante e confirmar que passa**

```bash
CARGO_BUILD_JOBS=1 cargo test -j1 --test log_convention_it 2>&1 | tail -10
```
Esperado: PASS.

- [ ] **Step 3: Conferir que `serde_json` ainda é usado em `main.rs`, ou remover o import**

```bash
grep -n "serde_json" src/main.rs
```
Se não sobrou nenhum uso, remova o import; se sobrou, deixe. O clippy pega import não usado, mas confira à mão para não deixar um `use` órfão que só o `--all-targets` acusaria.

- [ ] **Step 4: Verificar a saída real em JSON**

Não confie na leitura. Rode o binário com o formato JSON ligado e confira que os campos aparecem como campos:

```bash
QUARK_LOG_FORMAT=json CARGO_BUILD_JOBS=1 cargo run -j1 --bin quark 2>&1 | head -20
```

Procure uma linha de boot e confirme a forma `{"level":"INFO","fields":{"message":"...","campo":valor}}`, sem JSON escapado dentro de `message`. Se o binário exigir configuração para subir, use o mínimo necessário e diga no relatório o que precisou. Cole a linha real no relatório.

- [ ] **Step 5: Gate**

```bash
cargo fmt --check
CARGO_BUILD_JOBS=1 cargo clippy -j1 --all-targets -- -D warnings
CARGO_BUILD_JOBS=1 cargo test -j1 --lib
CARGO_BUILD_JOBS=1 cargo test -j1 --test log_convention_it
```

- [ ] **Step 6: Commit**

```bash
git add src/main.rs tests/log_convention_it.rs
git commit -m "fix(logs): campos nativos de tracing nos oito sitios do main.rs (LUC-142)

O LUC-130 converteu os modulos de biblioteca e deixou o main.rs para tras. Os
oito sitios restantes montavam JSON a mao dentro da macro, o que produz um JSON
serializado dentro do campo message: os dados viram texto e param de ser
agregaveis, que e justamente o que emitir JSON deveria resolver.

Seis dos oito logavam erro como info. A convencao do repo pede warn para todo
caminho fail-open, e um alerta montado sobre o nivel nunca dispararia para
nenhuma dessas falhas.

Entra tambem uma invariante sobre o fonte que falha se o padrao voltar. Nao
havia nada checando isso, e foi por isso que o main.rs ficou para tras sem
ninguem notar.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

## Auto-revisão do plano

| Requisito do spec | Task |
|---|---|
| Oito sítios com campos nativos | 2 |
| Níveis corrigidos (6 viram `warn!`) | 2 |
| Os dois casos de boot/sucesso seguem `info!` | 2 |
| Invariante que impede a volta | 1 |
| Invariante asserta zero, não contagem | 1 |
| Verificação da saída real em JSON | 2, Step 4 |

**Risco conhecido:** o teste da Task 1 varre texto, então ele acusaria um `json!` que aparecesse dentro de uma macro de log em contexto legítimo (por exemplo, um teste que monta payload). Hoje não existe nenhum caso assim em `src/main.rs` nem em `src/lib.rs`, que são os únicos arquivos varridos. Se algum dia existir, a saída do teste diz exatamente qual linha, e a decisão fica com quem escreveu.

**Nota sobre a lista `SOURCES`:** ela é explícita em vez de glob de propósito. Um glob daria falsa sensação de cobertura total sem ninguém conferir o que entrou. Os outros arquivos de `src/` já foram convertidos pelo LUC-130 e estão limpos; se quiser cobrir todos, adicione-os à lista e o teste dirá se algum regrediu.
