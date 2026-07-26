# LUC-140 + LUC-141 — Redação da URL de webhook e falha permanente (plano)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A URL de webhook deixa de ser impressa crua no log por construção do tipo, e um destino que responde 404 ou 410 é confirmado uma vez e então desativado, em vez de ser retentado para sempre.

**Architecture:** Um newtype `WebhookUrl` com `Display`/`Debug` redigidos e `#[serde(transparent)]` substitui a `String` do campo `url`. Como `reqwest` não implementa `IntoUrl` para o newtype, os pontos que montam a requisição param de compilar até chamar `.expose()`, enquanto os sítios de log seguem compilando e passam a redigir sozinhos. Em paralelo, a política de retry passa a classificar a resposta: 404 e 410 recebem orçamento de 2 tentativas em vez de 8 (ou 3, no caminho em memória), e esgotar o orçamento permanente desativa a subscription gravando o motivo numa coluna nova.

**Tech Stack:** Rust (axum 0.8, tokio, reqwest, serde, thiserror, tracing), heed 0.22 (LMDB), sqlx 0.9 (Postgres), React + TypeScript + Vite + TanStack Query (painel), Vitest + Testing Library (frontend).

**Spec:** `docs/specs/2026-07-26-webhooks-url-redaction-and-permanent-failure-design.md`

## Global Constraints

- Nada pode tocar o hot path do redirect. `link.clicked` é emitido no caminho síncrono do redirect e não pode ganhar trabalho novo.
- `src/codec.rs` e `src/permute.rs` são intocáveis.
- Gate de cada task: `cargo fmt`, `cargo clippy --all-targets -- -D warnings` (zero warnings), testes de lib, e testes gated no Postgres via `QUARK_TEST_DATABASE_URL` com usuário **não-superusuário** (RLS não se aplica a superusuário).
- Sempre `-j1` / `CARGO_BUILD_JOBS=1`. No Bash, `export PATH="$HOME/.cargo/bin:$PATH"`.
- Sem `CREATE INDEX CONCURRENTLY`.
- **Nenhuma crate nova.** `tracing-subscriber` já é dependência e é o suficiente para o teste de captura de log.
- Convenção de níveis (`.claude/skills/quark-rust/references/errors-and-observability.md`): `error!` para o que precisa de atenção, `warn!` para todo caminho fail-open, `info!` para boot e lifecycle. Campos: `%value` para `Display`, `?value` para `Debug`, `field = value` para primitivos. A mensagem é frase humana curta; o dado vai nos campos.
- Prosa de commit, doc e PR segue as regras de avoid-ai-writing: sem travessão, técnico e direto.

---

### Task 1: Newtype `WebhookUrl` com `Display` redigido

O coração da LUC-140. Depois desta task o vazamento é impossível por construção, e o resto do repo compila.

**Files:**
- Modify: `src/webhooks/mod.rs` (define o tipo; troca `pub url: String` na linha 121)
- Modify: `src/webhooks/delivery.rs` (2 sítios de `.post(&sub.url)`: linhas 412 e 716; 10 sítios de log ficam como estão)
- Modify: `src/api/webhooks_api.rs` (linhas 53, 250, 291, 596, 654)
- Modify: `src/api/slack.rs` (linhas 128, 233)
- Modify: `src/store/postgres.rs` (bind e leitura da coluna `url` da tabela `webhooks`)
- Modify: `src/store/lmdb.rs` (só se algum ponto usar `sub.url` como `&str`; o blob é serde, então provavelmente nada muda)
- Test: `src/webhooks/mod.rs` (módulo `#[cfg(test)]` já existente no fim do arquivo)

**Interfaces:**
- Produces:
  ```rust
  pub struct WebhookUrl(String);
  impl WebhookUrl {
      pub fn new(raw: impl Into<String>) -> Self;
      pub fn expose(&self) -> &str;
      pub fn redacted(&self, id: u64) -> String;
  }
  // Display/Debug redigidos; Serialize/Deserialize transparentes;
  // Clone, PartialEq, Eq derivados. From<String> e From<&str>.
  ```
- Consumes: nada (primeira task).

- [ ] **Step 1: Escrever os testes que falham**

Em `src/webhooks/mod.rs`, dentro do `mod tests` existente:

```rust
const DISCORD: &str =
    "https://discord.com/api/webhooks/1234567890/aVerySecretTokenThatMustNeverLeak";

#[test]
fn display_redacts_the_token_and_keeps_the_host() {
    let url = WebhookUrl::new(DISCORD);
    let shown = format!("{url}");
    assert!(!shown.contains("aVerySecretTokenThatMustNeverLeak"), "vazou: {shown}");
    assert!(shown.contains("discord.com"), "sem host para diagnostico: {shown}");
}

#[test]
fn debug_redacts_the_token_too() {
    let url = WebhookUrl::new(DISCORD);
    let shown = format!("{url:?}");
    assert!(!shown.contains("aVerySecretTokenThatMustNeverLeak"), "vazou: {shown}");
}

#[test]
fn expose_returns_the_raw_url() {
    assert_eq!(WebhookUrl::new(DISCORD).expose(), DISCORD);
}

#[test]
fn serde_is_transparent_so_persisted_blobs_do_not_change() {
    let url = WebhookUrl::new(DISCORD);
    assert_eq!(serde_json::to_string(&url).unwrap(), format!("\"{DISCORD}\""));
    let back: WebhookUrl = serde_json::from_str(&format!("\"{DISCORD}\"")).unwrap();
    assert_eq!(back.expose(), DISCORD);
}

#[test]
fn legacy_subscription_blob_still_deserializes() {
    // O mesmo blob legado que o teste pre-existente da linha 507 usa.
    let legacy = r#"{"id":7,"url":"https://h/x","events":["link.created"],"secret":"s","active":true,"created":0}"#;
    let sub: WebhookSubscription = serde_json::from_str(legacy).unwrap();
    assert_eq!(sub.url.expose(), "https://h/x");
}

#[test]
fn redacted_includes_the_subscription_id() {
    let shown = WebhookUrl::new(DISCORD).redacted(42);
    assert!(shown.contains("42"), "sem id para achar a linha no banco: {shown}");
    assert!(!shown.contains("aVerySecretTokenThatMustNeverLeak"));
}
```

- [ ] **Step 2: Rodar e confirmar que falha**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
CARGO_BUILD_JOBS=1 cargo test -j1 --lib webhooks::tests -- --nocapture
```
Esperado: erro de compilação, `cannot find type WebhookUrl in this scope`.

- [ ] **Step 3: Implementar o tipo**

Em `src/webhooks/mod.rs`, acima de `WebhookSubscription`:

```rust
/// A URL de destino de um webhook. Para Slack, Discord, Telegram e a maioria
/// dos conectores genericos (Make, Zapier, n8n) **a URL e a credencial**: o
/// token vive no path, e quem tem a URL publica no canal. Por isso `Display` e
/// `Debug` sao redigidos: qualquer `tracing::warn!(url = %sub.url, ...)`
/// imprime host e nada mais.
///
/// O valor cru so sai por `expose()`, que ecoa o `ExposeSecret` do `secrecy`
/// usado em `AppState::signing_key`. `reqwest` nao implementa `IntoUrl` para
/// este tipo, entao todo ponto que monta a requisicao precisa de `expose()`
/// explicito e o vazamento vira erro de compilacao, nao disciplina de revisao.
///
/// `serde` e transparente: o valor persistido em LMDB e Postgres, e o devolvido
/// por `GET /admin/webhooks`, seguem sendo a string crua. Nao ha migracao de
/// dado nem mudanca de contrato de API.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WebhookUrl(String);

impl WebhookUrl {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// O valor cru. Use so onde a URL precisa mesmo ir para a rede.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Forma redigida com o id da subscription, que e o que o operador precisa
    /// para achar a linha no banco.
    pub fn redacted(&self, id: u64) -> String {
        format!("{}#{id}", self.host_or_placeholder())
    }

    fn host_or_placeholder(&self) -> &str {
        crate::abuse::extract_host(&self.0)
            .map_or("<url invalida>", |_| {
                // `extract_host` devolve String; reaproveitar o slice do proprio
                // campo evita alocar no caminho de log.
                let after_scheme = self.0.split("://").nth(1).unwrap_or(&self.0);
                after_scheme.split('/').next().unwrap_or("<url invalida>")
            })
    }
}

impl std::fmt::Display for WebhookUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/…", self.host_or_placeholder())
    }
}

impl std::fmt::Debug for WebhookUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WebhookUrl({self})")
    }
}

impl From<String> for WebhookUrl {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for WebhookUrl {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}
```

Nota para quem implementa: se `crate::abuse::extract_host` não estiver acessível de `webhooks::mod`, extraia o host com o `split` puro (sem chamar `extract_host`) e ajuste o teste `display_redacts_the_token_and_keeps_the_host` para continuar exigindo `discord.com`. O importante é nunca imprimir o path.

Troque o campo na linha 121:

```rust
    pub url: WebhookUrl,
```

- [ ] **Step 4: Consertar o que parou de compilar**

```bash
CARGO_BUILD_JOBS=1 cargo build -j1 2>&1 | grep -E "^error" | head -30
```

Cada erro é um ponto que usa a URL crua. Regra: onde a URL vai para a rede ou para validação, `.expose()`; onde ela é construída a partir de entrada do usuário, `WebhookUrl::new(...)` ou `.into()`.

Pontos esperados:
- `src/webhooks/delivery.rs:412` → `.post(sub.url.expose())`
- `src/webhooks/delivery.rs:716` → `.post(sub.url.expose())`
- `src/webhooks/delivery.rs:309` e `:629` → `extract_host(sub.url.expose())`
- `src/api/webhooks_api.rs:250` → `url: WebhookUrl::new(req.url)`
- `src/api/webhooks_api.rs:291` → `sub.url = WebhookUrl::new(url)`
- `src/api/webhooks_api.rs:596` → `extract_host(sub.url.expose())`
- `src/api/webhooks_api.rs:654` → `.post(sub.url.expose())`
- `src/api/webhooks_api.rs:53` → a resposta serializa `WebhookUrl` transparente; se o campo do DTO for `String`, use `s.url.expose().to_string()`
- `src/api/slack.rs:128` e `:233` → comparação de igualdade passa a ser entre `WebhookUrl`, ou compare `.expose()`
- `src/store/postgres.rs` → o `bind` da coluna `url` vira `.bind(sub.url.expose())` e a leitura vira `WebhookUrl::new(row.get::<String, _>("url"))`

**Não** altere os 10 sítios de log. Eles seguem `url = %sub.url` e passam a redigir sozinhos: é essa a prova de que o mecanismo funciona.

- [ ] **Step 5: Rodar os testes e o gate**

```bash
CARGO_BUILD_JOBS=1 cargo test -j1 --lib webhooks 2>&1 | tail -20
cargo fmt --check
CARGO_BUILD_JOBS=1 cargo clippy -j1 --all-targets -- -D warnings 2>&1 | tail -20
```
Esperado: todos os testes de `webhooks` passam, fmt limpo, clippy sem warning.

- [ ] **Step 6: Rodar a suíte inteira, incluindo os testes gated**

```bash
CARGO_BUILD_JOBS=1 QUARK_TEST_DATABASE_URL="$QUARK_TEST_DATABASE_URL" cargo test -j1 2>&1 | tail -30
```
Esperado: zero falhas. Atenção especial a `webhooks_store_it.rs` e `webhooks_api_it.rs`, que fazem round-trip da subscription.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(webhooks): WebhookUrl com Display redigido (LUC-140)

A URL de destino e a credencial em Discord, Slack, Telegram e nos conectores
genericos: o token vive no path. O newtype redige Display e Debug, entao os 10
sitios de log em delivery.rs param de vazar sem que nenhum deles seja alterado.

serde e transparente, entao storage e API seguem carregando a string crua: nao
ha migracao de dado nem mudanca de contrato. Como reqwest nao implementa
IntoUrl para o newtype, todo ponto que monta a requisicao precisa de expose()
explicito, e reintroduzir o vazamento passa a ser erro de compilacao.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Fechar o vazamento por `error = %e` e provar com captura de `tracing`

**Correção do spec, achada na Task 1.** O spec afirmava que o newtype torna o vazamento impossível por construção. Isso vale para `url = %sub.url`, mas **não** para `error = %e`: o `Display` de `reqwest::Error` embute a URL completa com token. O comentário em `delivery.rs:436-442` documenta exatamente isso e loga o erro cheio de propósito, aplicando `without_url()` só ao detalhe persistido.

Sobram dois sítios vazando, que a Task 1 deliberadamente não tocou:
- `src/webhooks/delivery.rs:443` (`deliver_one`, erro de transporte)
- `src/webhooks/delivery.rs:732` (`post_once`, erro de transporte no relay)

Esta task fecha os dois e prova com o teste de captura.

**Atenção ao desenho do teste:** um servidor que responde 500 **não** exercita esse caminho. 500 é resposta HTTP válida e cai no ramo `Ok(resp)`, que nunca constrói um `reqwest::Error`. O teste precisa forçar **erro de transporte** — apontar para uma porta fechada é o jeito mais simples e determinístico.

**Files:**
- Modify: `src/webhooks/delivery.rs:443` e `:732`
- Create: `tests/webhooks_log_redaction_it.rs`

**Interfaces:**
- Consumes: `quark::webhooks::WebhookUrl` (Task 1), `tests/common/mod.rs` `TestState` builder.
- Produces: nada.

- [ ] **Step 1: Escrever o teste que falha**

```rust
//! O token de um webhook de canal vive na URL. Este teste captura a saida real
//! de `tracing` durante uma entrega que falha e prova que ele nao aparece.
//! Guarda de regressao para a LUC-140: o teste de tipo em `webhooks::tests`
//! prova o `Display`, este prova o caminho de entrega inteiro.

mod common;

use std::io::Write;
use std::sync::{Arc, Mutex};

/// Writer em memoria para o subscriber de teste.
#[derive(Clone, Default)]
struct CapturedLog(Arc<Mutex<Vec<u8>>>);

impl CapturedLog {
    fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl Write for CapturedLog {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLog {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

const SECRET_TOKEN: &str = "aVerySecretTokenThatMustNeverLeak";

#[tokio::test]
async fn a_failing_delivery_never_logs_the_url_token() {
    let captured = CapturedLog::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(captured.clone())
        .with_max_level(tracing::Level::TRACE)
        .finish();

    // Porta fechada de proposito: forca um `reqwest::Error` de transporte, que
    // e o unico caminho que constroi o erro cujo `Display` embute a URL. Um
    // servidor que responde 500 NAO serve aqui: 500 e resposta valida, cai no
    // ramo `Ok(resp)`, e o teste passaria com o vazamento vivo.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let url = format!("http://{addr}/hook/{SECRET_TOKEN}");

    tracing::subscriber::with_default(subscriber, || {
        // O corpo assincrono roda dentro do escopo do subscriber.
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                // Dispara a entrega pelo caminho publico do modulo. O helper
                // exato depende do que `webhooks::delivery` expoe para teste;
                // use o mesmo seam que `worker_refuses_internal_destination`
                // usa em `tests/webhooks_it.rs`.
                common::deliver_once_for_test(&url, SECRET_TOKEN).await;
            })
        });
    });

    let log = captured.contents();
    assert!(
        !log.contains(SECRET_TOKEN),
        "o token vazou no log:\n{log}"
    );
    assert!(
        log.contains(&addr.ip().to_string()),
        "o log perdeu o host e ficou inutil para diagnostico:\n{log}"
    );
}
```

Nota para quem implementa: `common::deliver_once_for_test` não existe ainda. Antes de escrever o teste, abra `tests/webhooks_it.rs` e veja qual seam já é usado para exercitar HTTP real (o spec aponta `deliver_to_matching_guarded`, que recebe o predicado de SSRF injetado justamente para testes com servidor local). Use esse seam direto no teste em vez de criar um helper novo, se ele já for `pub(crate)` o suficiente. Se precisar de um helper, coloque em `tests/common/mod.rs`.

- [ ] **Step 2: Rodar e confirmar que falha**

```bash
CARGO_BUILD_JOBS=1 cargo test -j1 --test webhooks_log_redaction_it 2>&1 | tail -20
```
Esperado: falha de compilação no helper, ou falha de asserção se o teste for escrito antes da Task 1.

- [ ] **Step 3: Fazer passar**

Primeiro confirme que o teste **falha de verdade** com o código atual: ele tem que acusar o token vindo do `error = %e`, não passar de graça. Se passar antes da correção, o teste não está exercitando o caminho de transporte e precisa ser consertado antes de qualquer outra coisa.

Depois, feche os dois sítios:

```rust
// src/webhooks/delivery.rs, em deliver_one (linha ~443)
tracing::warn!(
    error = %e.without_url(),
    url = %sub.url,
    attempt = attempt + 1,
    "webhook delivery failed"
);

// src/webhooks/delivery.rs, em post_once (linha ~732)
tracing::warn!(error = %e.without_url(), url = %sub.url, "relayed webhook delivery failed");
```

Atualize o comentário de `delivery.rs:436-442`: ele hoje justifica logar o erro cheio ("Log the full error first (operators need it)"), e essa justificativa deixou de valer. O `url = %sub.url` redigido já dá ao operador o host e o id da subscription, que é o que ele precisa para achar a linha no banco. O resto da URL nunca foi diagnóstico, era só a credencial.

- [ ] **Step 4: Rodar o gate**

```bash
CARGO_BUILD_JOBS=1 cargo test -j1 --test webhooks_log_redaction_it 2>&1 | tail -10
cargo fmt --check
CARGO_BUILD_JOBS=1 cargo clippy -j1 --all-targets -- -D warnings 2>&1 | tail -10
```

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "test(webhooks): captura de tracing prova que o token nao vaza (LUC-140)

Guarda de regressao de ponta a ponta: instala um subscriber com writer em
memoria, forca uma entrega que falha contra um servidor local que responde 500,
e asserta que o token da URL nao aparece na saida capturada e que o host
aparece, para o log seguir servindo a diagnostico.

Sem crate nova: tracing-subscriber ja e dependencia do projeto.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: `disabled_reason` no modelo e `disable_webhook` no `Store`

Base da LUC-141. Só o estado persistido, sem ninguém chamando ainda.

**Files:**
- Modify: `src/webhooks/mod.rs` (campo novo em `WebhookSubscription`, depois da linha 150)
- Modify: `src/store/mod.rs` (método novo no trait `Store`, ao lado de `record_webhook_health` na linha 430)
- Modify: `src/store/lmdb.rs` (impl, ao lado de `record_webhook_health` na linha 555)
- Modify: `src/store/postgres.rs` (impl ao lado da linha 1526; DDL ao lado da linha 730; leitura da linha em `WebhookSubscription`)
- Test: `tests/webhooks_store_it.rs`

**Interfaces:**
- Consumes: `WebhookUrl` (Task 1).
- Produces:
  ```rust
  // src/webhooks/mod.rs, em WebhookSubscription:
  #[serde(default)]
  pub disabled_reason: Option<String>,

  // src/store/mod.rs, no trait Store:
  async fn disable_webhook(
      &self,
      tenant: TenantId,
      id: u64,
      reason: &str,
  ) -> Result<(), StoreError>;
  ```

- [ ] **Step 1: Escrever os testes que falham**

Em `tests/webhooks_store_it.rs`:

```rust
#[tokio::test]
async fn disable_webhook_sets_inactive_with_reason_pg() {
    let Some(store) = pg_store().await else {
        return;
    };
    let tenant = TenantId(1);
    let id = store.next_webhook_id(tenant).await.unwrap();
    let sub = WebhookSubscription {
        id,
        url: quark::webhooks::WebhookUrl::new("https://example.com/hook"),
        events: vec![EventType::LinkCreated],
        secret: "s".into(),
        active: true,
        created: 0,
        kind: SubscriptionKind::Generic,
        label: None,
        connector_id: None,
        external_id: None,
        last_delivery_at: None,
        last_delivery_status: Default::default(),
        disabled_reason: None,
    };
    store.put_webhook(tenant, &sub).await.unwrap();

    store
        .disable_webhook(tenant, id, "status 410")
        .await
        .unwrap();

    let got = store.get_webhook(tenant, id).await.unwrap().unwrap();
    assert!(!got.active, "a subscription deveria ter sido desativada");
    assert_eq!(got.disabled_reason.as_deref(), Some("status 410"));
}

#[tokio::test]
async fn reactivating_clears_the_disabled_reason_pg() {
    let Some(store) = pg_store().await else {
        return;
    };
    let tenant = TenantId(1);
    let id = store.next_webhook_id(tenant).await.unwrap();
    let mut sub = /* mesmo literal do teste acima, com este id */;
    store.put_webhook(tenant, &sub).await.unwrap();
    store.disable_webhook(tenant, id, "status 404").await.unwrap();

    sub.active = true;
    sub.disabled_reason = None;
    store.put_webhook(tenant, &sub).await.unwrap();

    let got = store.get_webhook(tenant, id).await.unwrap().unwrap();
    assert!(got.active);
    assert_eq!(got.disabled_reason, None, "reativar tem que limpar o motivo");
}
```

Nota: copie o literal completo de `WebhookSubscription` no segundo teste em vez do comentário — os helpers de construção existentes em `webhooks_store_it.rs` provavelmente já cobrem isso, use-os se existirem.

Adicione o equivalente para LMDB no `#[cfg(test)]` de `src/store/lmdb.rs` se já houver testes de webhook lá; senão, um teste em `tests/` que rode contra o backend LMDB padrão.

- [ ] **Step 2: Rodar e confirmar que falha**

```bash
CARGO_BUILD_JOBS=1 QUARK_TEST_DATABASE_URL="$QUARK_TEST_DATABASE_URL" cargo test -j1 --test webhooks_store_it 2>&1 | tail -20
```
Esperado: erro de compilação, `no method named disable_webhook` e `missing field disabled_reason`.

- [ ] **Step 3: Adicionar o campo**

Em `src/webhooks/mod.rs`, depois de `last_delivery_status`:

```rust
    /// Motivo pelo qual o sistema desativou esta subscription, `None` quando
    /// `active` reflete escolha do usuario. Preenchido quando um destino
    /// responde de forma permanente (404/410) e a tentativa de confirmacao
    /// tambem falha. E o que permite o painel distinguir "eu pausei" de "o
    /// sistema desativou": derivar isso de `!active && status == error` mentiria
    /// quando o usuario pausa na mao um webhook que ja vinha falhando.
    #[serde(default)]
    pub disabled_reason: Option<String>,
```

- [ ] **Step 4: Adicionar o método ao trait e implementar nos dois backends**

Em `src/store/mod.rs`, junto de `record_webhook_health`:

```rust
    /// Desativa uma subscription e registra por que. Usado quando o destino
    /// responde de forma permanente (404/410) e a tentativa de confirmacao
    /// tambem falha: sem isso o dispatcher retenta um destino morto para
    /// sempre. Reativar e responsabilidade do usuario pelo painel, e limpa o
    /// motivo (ver `put_webhook`).
    async fn disable_webhook(
        &self,
        tenant: TenantId,
        id: u64,
        reason: &str,
    ) -> Result<(), StoreError>;
```

Em `src/store/lmdb.rs`, no mesmo padrão de leitura-modificação-escrita de `record_webhook_health`:

```rust
    async fn disable_webhook(
        &self,
        tenant: TenantId,
        id: u64,
        reason: &str,
    ) -> Result<(), StoreError> {
        let mut wtxn = self.env.write_txn()?;
        let key = tkey_id(tenant, id);
        if let Some(bytes) = self.webhooks.get(&wtxn, &key)? {
            let mut sub: WebhookSubscription = serde_json::from_slice(bytes)?;
            sub.active = false;
            sub.disabled_reason = Some(reason.to_string());
            let out = serde_json::to_vec(&sub)?;
            self.webhooks.put(&mut wtxn, &key, &out)?;
            wtxn.commit()?;
        }
        Ok(())
    }
```

Em `src/store/postgres.rs`, junto da DDL da tabela `webhooks` (depois da linha 730):

```rust
                // Motivo de desativacao automatica (LUC-141): distingue
                // "usuario pausou" de "o sistema desativou por destino morto".
                // Nullable; linhas pre-existentes ficam NULL, que e o mesmo que
                // "nunca foi desativado pelo sistema".
                "ALTER TABLE webhooks ADD COLUMN IF NOT EXISTS disabled_reason TEXT",
```

E a impl, no padrão de `record_webhook_health`:

```rust
    async fn disable_webhook(
        &self,
        tenant: TenantId,
        id: u64,
        reason: &str,
    ) -> Result<(), StoreError> {
        with_write!(self, tenant, |c| {
            sqlx::query(
                "UPDATE webhooks SET active=false, disabled_reason=$1 WHERE tenant_id=$2 AND id=$3",
            )
            .bind(reason)
            .bind(tenant.0 as i64)
            .bind(id as i64)
            .execute(&mut *c)
            .await
        });
        Ok(())
    }
```

Atualize também, no mesmo arquivo, o `INSERT`/`UPSERT` de `put_webhook` e o `SELECT` que monta `WebhookSubscription` para carregarem `disabled_reason`. Sem isso, `put_webhook` não limpa o motivo ao reativar e o segundo teste falha.

- [ ] **Step 5: Corrigir os construtores que agora faltam campo**

```bash
CARGO_BUILD_JOBS=1 cargo build -j1 --all-targets 2>&1 | grep -E "missing field" | head -20
```
Adicione `disabled_reason: None` em cada literal de `WebhookSubscription` em `src/` e `tests/`.

- [ ] **Step 6: Rodar os testes e o gate**

```bash
CARGO_BUILD_JOBS=1 QUARK_TEST_DATABASE_URL="$QUARK_TEST_DATABASE_URL" cargo test -j1 --test webhooks_store_it 2>&1 | tail -20
cargo fmt --check
CARGO_BUILD_JOBS=1 cargo clippy -j1 --all-targets -- -D warnings 2>&1 | tail -10
```
Esperado: os dois testes novos passam. Se `QUARK_TEST_DATABASE_URL` não estiver setado eles retornam cedo sem rodar; confirme que rodaram de verdade procurando os nomes na saída.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(store): disabled_reason e disable_webhook (LUC-141)

Estado persistido para a desativacao automatica de destino morto. A coluna
segue o padrao de evolucao de schema do repo (ALTER TABLE ADD COLUMN IF NOT
EXISTS no boot), e o campo tem serde(default) como os outros adicionados depois
da v1, entao blobs LMDB antigos deserializam sem mudanca.

Coluna dedicada em vez de derivar de !active && status == error no frontend:
a derivacao mentiria quando o usuario pausa na mao um webhook que ja falhava.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Classificar a falha e desativar o destino morto

O comportamento da LUC-141, nos dois caminhos de entrega.

**Files:**
- Modify: `src/webhooks/delivery.rs` (constantes junto da linha 487; `deliver_one` linhas 399-480; `deliver_claimed` linhas 587-706; `post_once` linhas 710-736)
- Test: `tests/webhooks_it.rs`

**Interfaces:**
- Consumes: `disable_webhook` e `disabled_reason` (Task 3); `WebhookUrl::expose` (Task 1).
- Produces:
  ```rust
  // src/webhooks/delivery.rs
  pub const PERMANENT_DELIVERY_ATTEMPTS: u32 = 2;
  pub(crate) fn is_permanent(status: u16) -> bool;

  /// Substitui o `bool` de `post_once`.
  pub(crate) enum AttemptOutcome {
      Success,
      Permanent(u16),
      Transient,
  }
  ```

- [ ] **Step 1: Escrever os testes que falham**

Em `tests/webhooks_it.rs`. Use o servidor de teste local que o arquivo já monta para os outros casos, com um contador de requisições compartilhado (`Arc<AtomicUsize>`) e status configurável.

```rust
#[tokio::test]
async fn a_410_destination_is_confirmed_once_then_disabled() {
    // Servidor que responde 410 sempre, contando as requisicoes.
    let (url, hits) = spawn_status_server(410).await;
    let (store, tenant, sub_id) = seed_active_webhook(&url).await;

    deliver_one_for_test(&store, tenant, sub_id).await;

    assert_eq!(
        hits.load(Ordering::SeqCst),
        2,
        "410 deve ter a tentativa original mais uma confirmacao, nunca o orcamento inteiro"
    );
    let got = store.get_webhook(tenant, sub_id).await.unwrap().unwrap();
    assert!(!got.active, "410 confirmado tem que desativar");
    assert_eq!(got.disabled_reason.as_deref(), Some("status 410"));
}

#[tokio::test]
async fn a_404_destination_is_confirmed_once_then_disabled() {
    let (url, hits) = spawn_status_server(404).await;
    let (store, tenant, sub_id) = seed_active_webhook(&url).await;

    deliver_one_for_test(&store, tenant, sub_id).await;

    assert_eq!(hits.load(Ordering::SeqCst), 2);
    let got = store.get_webhook(tenant, sub_id).await.unwrap().unwrap();
    assert!(!got.active);
    assert_eq!(got.disabled_reason.as_deref(), Some("status 404"));
}

#[tokio::test]
async fn a_404_that_recovers_on_the_confirmation_attempt_is_not_disabled() {
    // Este e o teste que prova que a tentativa de confirmacao serve para algo:
    // um 404 momentaneo de janela de deploy nao pode matar a integracao.
    let (url, hits) = spawn_status_sequence_server(vec![404, 200]).await;
    let (store, tenant, sub_id) = seed_active_webhook(&url).await;

    deliver_one_for_test(&store, tenant, sub_id).await;

    assert_eq!(hits.load(Ordering::SeqCst), 2);
    let got = store.get_webhook(tenant, sub_id).await.unwrap().unwrap();
    assert!(got.active, "um 404 que se recupera na confirmacao nao pode desativar");
    assert_eq!(got.disabled_reason, None);
}

#[tokio::test]
async fn a_503_destination_keeps_the_full_transient_budget_and_is_not_disabled() {
    let (url, hits) = spawn_status_server(503).await;
    let (store, tenant, sub_id) = seed_active_webhook(&url).await;

    deliver_one_for_test(&store, tenant, sub_id).await;

    assert_eq!(
        hits.load(Ordering::SeqCst),
        quark::webhooks::delivery::DELIVERY_ATTEMPTS as usize,
        "5xx e transitorio e mantem o orcamento de hoje"
    );
    let got = store.get_webhook(tenant, sub_id).await.unwrap().unwrap();
    assert!(got.active, "5xx nunca desativa: o destino pode voltar");
    assert_eq!(got.disabled_reason, None);
}
```

Repita os quatro casos para o caminho do relay durável, no arquivo de teste que já cobre o relay (`tests/webhooks_relay_it.rs` se existir, senão o mesmo `webhooks_it.rs`), com a diferença de que lá o orçamento transitório é `MAX_DELIVERY_ATTEMPTS` (8) e as tentativas são distribuídas por chamadas separadas de `deliver_claimed`, não por um loop interno. Para o relay, o assert de contagem vira: depois da segunda chamada com 410, a subscription está desativada e a linha está `dead`.

Nota: `spawn_status_server`, `spawn_status_sequence_server`, `seed_active_webhook` e `deliver_one_for_test` são helpers de teste. Verifique o que `tests/webhooks_it.rs` já tem antes de escrever qualquer um deles; o arquivo já monta servidor local para `worker_refuses_internal_destination` e afins.

- [ ] **Step 2: Rodar e confirmar que falha**

```bash
CARGO_BUILD_JOBS=1 cargo test -j1 --test webhooks_it 2>&1 | tail -30
```
Esperado: os quatro testes falham. Os de 410/404 falham na contagem (hoje seriam 3, o orçamento inteiro) e no `active`, que segue `true`.

- [ ] **Step 3: Implementar a classificação**

Em `src/webhooks/delivery.rs`, junto de `MAX_DELIVERY_ATTEMPTS`:

```rust
/// Orcamento de tentativas para uma resposta permanente: a original mais uma
/// confirmacao. Nao e 1 porque um 404 momentaneo de janela de deploy ou de
/// proxy nao pode matar a integracao do cliente; nao e o orcamento cheio porque
/// um destino de fato removido nao volta, e retenta-lo e o loop que esta issue
/// existe para acabar.
pub const PERMANENT_DELIVERY_ATTEMPTS: u32 = 2;

/// `404` e `410` significam que o destino nao existe mais: `410 Gone` e
/// literalmente o codigo para "isto foi removido e nao volta". `400` e `422`
/// foram deixados de fora de proposito: um `422` normalmente diz que o *nosso*
/// payload esta errado, e desativar a integracao do cliente por bug nosso e o
/// pior desfecho possivel. `429`, `5xx`, timeout e erro de transporte seguem
/// transitorios.
pub(crate) fn is_permanent(status: u16) -> bool {
    matches!(status, 404 | 410)
}
```

- [ ] **Step 4: Aplicar no caminho em memória (`deliver_one`)**

O loop das linhas 410-456 passa a ter teto variável. Substitua a condição fixa por um teto que encolhe quando a resposta é permanente:

```rust
    let mut outcome = crate::health::HealthStatus::Error("no attempt".into());
    let mut budget = DELIVERY_ATTEMPTS;
    let mut permanent_status: Option<u16> = None;
    let mut attempt = 0;
    while attempt < budget {
        // ... monta e envia o request exatamente como hoje ...
        match res {
            Ok(resp) if resp.status().is_success() => {
                outcome = crate::health::HealthStatus::Ok;
                permanent_status = None;
                break;
            }
            Ok(resp) => {
                let code = resp.status().as_u16();
                outcome = crate::health::HealthStatus::Error(format!("status {code}"));
                if is_permanent(code) {
                    permanent_status = Some(code);
                    budget = PERMANENT_DELIVERY_ATTEMPTS;
                } else {
                    permanent_status = None;
                }
                tracing::warn!(
                    status = code,
                    url = %sub.url,
                    attempt = attempt + 1,
                    "webhook delivery returned a non-2xx status"
                );
            }
            Err(e) => {
                // comentario existente sobre o detalhe persistido segue valendo
                tracing::warn!(error = %e, url = %sub.url, attempt = attempt + 1, "webhook delivery failed");
                outcome = crate::health::HealthStatus::Error(e.without_url().to_string());
                permanent_status = None;
            }
        }
        attempt += 1;
        if attempt < budget {
            tokio::time::sleep(backoff_with_jitter(attempt - 1)).await;
        }
    }
```

Depois do loop, antes do registro de saúde:

```rust
    if let Some(code) = permanent_status {
        let reason = format!("status {code}");
        tracing::warn!(
            webhook_id = sub.id,
            status = code,
            url = %sub.url,
            "webhook destination is gone, disabling the subscription"
        );
        if let Err(e) = store.disable_webhook(ev.tenant_id, sub.id, &reason).await {
            tracing::warn!(error = %e, webhook_id = sub.id, "webhook disable write failed");
        }
    }
```

Atenção ao hot path: `link.clicked` já é excluído do registro de saúde nas linhas 472-479 porque é emitido no caminho síncrono do redirect. **A desativação tem que respeitar a mesma exclusão** — envolva o bloco acima na mesma condição `if ev.event_type != EventType::LinkClicked`, ou o redirect passa a escrever no store.

- [ ] **Step 5: Aplicar no caminho do relay**

`post_once` deixa de devolver `bool`:

```rust
/// Resultado de uma tentativa. `Permanent` carrega o status para o motivo de
/// desativacao e para encurtar o orcamento.
pub(crate) enum AttemptOutcome {
    Success,
    Permanent(u16),
    Transient,
}

async fn post_once(
    client: &reqwest::Client,
    sub: &WebhookSubscription,
    req: &OutgoingRequest,
) -> AttemptOutcome {
    let mut builder = client
        .post(sub.url.expose())
        .header("content-type", "application/json");
    for (name, value) in &req.extra_headers {
        builder = builder.header(*name, value);
    }
    match builder.body(req.body.clone()).send().await {
        Ok(resp) if resp.status().is_success() => AttemptOutcome::Success,
        Ok(resp) => {
            let code = resp.status().as_u16();
            tracing::warn!(status = code, url = %sub.url, "relayed webhook returned a non-2xx status");
            if is_permanent(code) {
                AttemptOutcome::Permanent(code)
            } else {
                AttemptOutcome::Transient
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, url = %sub.url, "relayed webhook delivery failed");
            AttemptOutcome::Transient
        }
    }
}
```

Em `deliver_claimed`, a linha 659 (`if post_once(...).await`) vira um `match`, e o teto da linha 690 passa a depender do resultado:

```rust
    let outcome = post_once(client, sub, &req).await;
    if matches!(outcome, AttemptOutcome::Success) {
        // bloco de sucesso existente, inalterado
        return;
    }

    // registro de saude existente, inalterado

    let attempts = delivery.attempts.saturating_add(1);
    let budget = match outcome {
        AttemptOutcome::Permanent(_) => PERMANENT_DELIVERY_ATTEMPTS,
        _ => MAX_DELIVERY_ATTEMPTS,
    };
    if attempts >= budget {
        if let AttemptOutcome::Permanent(code) = outcome {
            let reason = format!("status {code}");
            tracing::warn!(
                webhook_id = sub.id,
                status = code,
                url = %sub.url,
                "relayed webhook destination is gone, disabling the subscription"
            );
            if let Err(e) = store.disable_webhook(delivery.tenant_id, sub.id, &reason).await {
                tracing::warn!(error = %e, webhook_id = sub.id, "webhook disable write failed");
            }
        }
        tracing::error!(
            delivery_key = %delivery.delivery_key,
            attempts,
            "relayed webhook dead-lettered after exhausting its attempts"
        );
        mark_dead_logged(store, delivery.id, attempts).await;
        return;
    }
    // agendamento de retry existente, inalterado
```

- [ ] **Step 6: Rodar os testes e o gate**

```bash
CARGO_BUILD_JOBS=1 cargo test -j1 --test webhooks_it 2>&1 | tail -30
CARGO_BUILD_JOBS=1 QUARK_TEST_DATABASE_URL="$QUARK_TEST_DATABASE_URL" cargo test -j1 2>&1 | tail -30
cargo fmt --check
CARGO_BUILD_JOBS=1 cargo clippy -j1 --all-targets -- -D warnings 2>&1 | tail -10
```
Esperado: os quatro testes novos passam nos dois caminhos, e a suíte inteira segue verde.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(webhooks): 404 e 410 confirmam uma vez e desativam (LUC-141)

A politica de retry tratava todo nao-2xx igual, entao um destino removido era
retentado ate esgotar o orcamento, evento apos evento, para sempre. 404 e 410
passam a ter orcamento de 2 tentativas: a original mais uma confirmacao, que
absorve o 404 momentaneo de janela de deploy sem reintroduzir o loop.

Confirmada a falha permanente, a subscription e desativada com o motivo
registrado. 400 e 422 ficaram de fora de proposito: um 422 costuma indicar que
o nosso payload esta errado, e desativar a integracao do cliente por bug nosso
e o pior desfecho. 429, 5xx e erro de transporte seguem com o backoff de hoje.

A desativacao respeita a mesma exclusao de link.clicked que o registro de
saude ja tinha: esse evento sai no caminho sincrono do redirect e nao pode
ganhar escrita no store.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Estado `disabled` na API e no painel

Fecha o aceite da LUC-141: o usuário precisa ver que o webhook morreu e por quê.

**Files:**
- Modify: `src/api/webhooks_api.rs` (DTO de resposta na linha 25 e mapeamento na linha 53)
- Modify: `web/src/lib/types.ts` (tipo `Webhook`, linhas 170-186)
- Modify: `web/src/routes/Webhooks.tsx` (`webhookHealth` linhas 85-89; `HEALTH_DOT_CLASS`/`HEALTH_LABEL_KEY` linhas 73-83; render do detalhe linhas 293-296)
- Modify: `web/src/locales/` (chaves de tradução nos dois idiomas)
- Test: `tests/webhooks_api_it.rs`, `web/src/routes/Webhooks.test.tsx`

**Interfaces:**
- Consumes: `disabled_reason` (Task 3).
- Produces: campo `disabled_reason: string | null` na resposta de `GET /admin/webhooks`.

- [ ] **Step 1: Escrever os testes que falham**

Em `tests/webhooks_api_it.rs`:

```rust
#[tokio::test]
async fn list_webhooks_exposes_disabled_reason() {
    let st = /* TestState builder como nos outros testes deste arquivo */;
    let (tenant, id) = /* semeia um webhook ativo */;
    st.store.disable_webhook(tenant, id, "status 410").await.unwrap();

    let body = /* GET /admin/webhooks e parseia o JSON */;
    let hook = &body.as_array().unwrap()[0];
    assert_eq!(hook["active"], serde_json::json!(false));
    assert_eq!(hook["disabled_reason"], serde_json::json!("status 410"));
}

#[tokio::test]
async fn a_user_paused_webhook_has_no_disabled_reason() {
    let st = /* TestState builder */;
    let (tenant, id) = /* semeia um webhook e o pausa via PATCH active=false */;

    let body = /* GET /admin/webhooks */;
    let hook = &body.as_array().unwrap()[0];
    assert_eq!(hook["active"], serde_json::json!(false));
    assert_eq!(
        hook["disabled_reason"],
        serde_json::Value::Null,
        "pausa manual nao pode se passar por desativacao automatica"
    );
}
```

Em `web/src/routes/Webhooks.test.tsx`:

```tsx
it("mostra o motivo quando o sistema desativou o webhook", async () => {
  renderWebhooks([
    makeWebhook({ active: false, disabled_reason: "status 410" }),
  ]);
  expect(await screen.findByText(/status 410/)).toBeInTheDocument();
});

it("distingue pausa manual de desativacao automatica", async () => {
  renderWebhooks([makeWebhook({ active: false, disabled_reason: null })]);
  const paused = await screen.findByText(/pausado/i);
  expect(paused).toBeInTheDocument();
  expect(screen.queryByText(/desativado/i)).not.toBeInTheDocument();
});
```

Nota: `makeWebhook` e `renderWebhooks` são helpers; use os que o arquivo de teste já tiver e só estenda o factory com o campo novo.

- [ ] **Step 2: Rodar e confirmar que falha**

```bash
CARGO_BUILD_JOBS=1 QUARK_TEST_DATABASE_URL="$QUARK_TEST_DATABASE_URL" cargo test -j1 --test webhooks_api_it 2>&1 | tail -20
cd web && npm test -- Webhooks 2>&1 | tail -20 && cd ..
```

- [ ] **Step 3: Expor o campo na API**

Em `src/api/webhooks_api.rs`, adicione ao struct de resposta (linha 25) e ao mapeamento (linha 53):

```rust
    disabled_reason: Option<String>,
```
```rust
                    disabled_reason: s.disabled_reason,
```

- [ ] **Step 4: Consumir no painel**

Em `web/src/lib/types.ts`, no tipo `Webhook`:

```ts
  /** Motivo de desativação automática. `null` quando o usuário pausou na mão. */
  disabled_reason: string | null;
```

Em `web/src/routes/Webhooks.tsx`, `webhookHealth` ganha o quarto estado. A ordem importa: `disabled` tem que ser checado antes de `paused`, senão todo webhook desativado renderiza como pausado.

```tsx
function webhookHealth(webhook: Webhook) {
  if (!webhook.active && webhook.disabled_reason) return "disabled";
  if (!webhook.active) return "paused";
  if (webhook.last_delivery_status?.state === "error") return "failing";
  return "active";
}
```

Adicione a entrada correspondente em `HEALTH_DOT_CLASS` e `HEALTH_LABEL_KEY`, e renderize `webhook.disabled_reason` junto do detalhe de erro que já existe nas linhas 293-296.

Chaves de tradução novas nos dois idiomas em `web/src/locales/`, seguindo o padrão das chaves de saúde já existentes.

- [ ] **Step 5: Rodar os testes e o gate completo**

```bash
CARGO_BUILD_JOBS=1 QUARK_TEST_DATABASE_URL="$QUARK_TEST_DATABASE_URL" cargo test -j1 2>&1 | tail -30
cargo fmt --check
CARGO_BUILD_JOBS=1 cargo clippy -j1 --all-targets -- -D warnings 2>&1 | tail -10
cd web && npm run lint && npm test 2>&1 | tail -20 && npx tsc --noEmit && cd ..
```
Esperado: suíte Rust inteira verde, frontend inteiro verde, tsc sem erro.

- [ ] **Step 6: Atualizar a documentação**

`docs/WEBHOOKS.md` e `docs/WEBHOOKS.PT_BR.md` descrevem a política de entrega (a seção citada no spec fica em torno da linha 239). Documente nos dois arquivos: que 404 e 410 são tratados como permanentes, que há uma tentativa de confirmação, que a subscription é desativada com motivo, e que reativar é pelo painel. Mantenha a linha de troca de idioma no topo de cada arquivo.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(web): estado disabled na tela de webhooks (LUC-141)

O painel tinha tres estados (pausado, falhando, ativo) e um webhook desativado
pelo sistema aparecia como pausado, sem dizer por que. Entra um quarto estado
com o motivo, checado antes de pausado para nao ser mascarado por ele.

Documenta a politica de falha permanente em WEBHOOKS.md e no twin PT_BR.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

## Auto-revisão do plano

**Cobertura do spec:**

| Requisito do spec | Task |
|---|---|
| Newtype `WebhookUrl`, Display redigido, serde transparente | 1 |
| `.expose()` obrigatório nos pontos de rede | 1 |
| Os 10 sítios de log não são alterados e passam a redigir | 1 |
| Teste de tipo (Display não contém token) | 1 |
| Teste de captura de `tracing` no caminho real | 2 |
| Coluna `disabled_reason` via ALTER TABLE IF NOT EXISTS | 3 |
| `disable_webhook` no trait e nos dois backends | 3 |
| 404 e 410 permanentes, 400/422 fora | 4 |
| Orçamento de 2 com tentativa de confirmação | 4 |
| Desativação ao confirmar a falha permanente | 4 |
| Transitório mantém 3 e 8 | 4 |
| Ambos os caminhos (memória e relay) | 4 |
| Teste: 404 que se recupera na confirmação não desativa | 4 |
| Quarto estado `disabled` no painel, distinto de `paused` | 5 |
| `disabled_reason` na resposta da API | 5 |
| Docs nos dois idiomas | 5 |

Sem lacunas.

**Consistência de tipos:** `disabled_reason` é `Option<String>` no Rust e `string | null` no TS em todas as tasks. `disable_webhook(tenant, id, reason: &str)` tem a mesma assinatura na Task 3 (definição) e na Task 4 (uso). `is_permanent(status: u16) -> bool` e `PERMANENT_DELIVERY_ATTEMPTS: u32` são definidos e usados só na Task 4. `WebhookUrl::expose()` é definido na Task 1 e usado nas Tasks 1 e 4.

**Riscos conhecidos para quem implementa:**

1. O maior risco da Task 1 é algum ponto de `sub.url` que hoje depende de coerção implícita para `&str` e que o compilador não aponta de forma óbvia. Rode `cargo build --all-targets`, não só `cargo build`: os testes também constroem `WebhookSubscription`.
2. Na Task 4, o erro mais fácil de cometer é passar a escrever no store no caminho de `link.clicked`. Esse evento sai no redirect síncrono. A exclusão já existe para o registro de saúde e tem que valer também para a desativação.
3. Na Task 5, checar `paused` antes de `disabled` mascara o estado novo inteiro e os testes de frontend passam a mentir. A ordem está explícita no código do plano.
