# async-stripe 1.0.0-rc: API real da linha v1 (rc.8)

Data da pesquisa: 2026-08-08. Versão pesquisada: **1.0.0-rc.8** (publicada em
2026-08-05, a mais recente da série no crates.io/docs.rs). A v1 é uma
reescrita completa: nada da linha 0.41 vale aqui. Tudo abaixo foi verificado
em docs.rs (páginas versionadas em 1.0.0-rc.8), no repositório
github.com/arlyon/async-stripe (branch master) e no crates.io. O que não foi
possível confirmar está marcado como tal.

Atenção com docs.rs: `docs.rs/async-stripe/latest` resolve para **0.41.0**
porque rc é pre-release. Sempre usar URLs com a versão explícita, por exemplo
`docs.rs/async-stripe/1.0.0-rc.8/stripe/`.

## 1. Crates e Cargo.toml

A v1 é um workspace fatiado: um crate de cliente (`async-stripe`) mais um
crate por área da API, cada um com uma feature por recurso (nenhuma ligada
por default). O nome no crates.io leva prefixo `async-stripe-`, mas o nome
de biblioteca no código é `stripe_*` (e o cliente é só `stripe`).

| crates.io | `use` no código | serve para | features necessárias |
|---|---|---|---|
| `async-stripe` | `stripe` | Client, ClientBuilder, StripeError | runtime/TLS (abaixo) |
| `async-stripe-core` | `stripe_core` | Customers | `customer` |
| `async-stripe-checkout` | `stripe_checkout` | Checkout Sessions | `checkout_session` |
| `async-stripe-billing` | `stripe_billing` | Subscriptions, Billing Portal | `subscription`, `billing_portal_session` |
| `async-stripe-product` | `stripe_product` | Prices (list por lookup_key) | `price` (e `product` se criar produtos) |
| `async-stripe-webhook` | `stripe_webhook` | verificação de assinatura + Event | `async-stripe-checkout` (para os payloads de checkout; uma feature por área de evento, ou `full`) |
| `async-stripe-types` | `stripe_types` | `Currency`, `Expandable`, `Timestamp`, `List` | nenhuma |
| `async-stripe-shared` | `stripe_shared` | structs compartilhadas (`Subscription`, `Price`, `SubscriptionStatus`) | vem transitivamente; declarar direto só se precisar nomear os tipos |

Runtime: o `async-stripe` tem 13 features; o default é `default-tls`
(tokio + hyper + native-tls). Para tokio + rustls, desligar defaults e usar
`rustls-tls-webpki-roots` (raízes webpki embutidas) ou `rustls-tls-native`
(certificados do sistema), mais um provider de crypto: `rustls-ring` ou
`rustls-aws-lc-rs`. Existem ainda `blocking`, `async-std-surf` e
`redact-generated-debug`.

```toml
[dependencies]
async-stripe = { version = "1.0.0-rc.8", default-features = false, features = ["rustls-tls-webpki-roots", "rustls-ring"] }
async-stripe-core = { version = "1.0.0-rc.8", features = ["customer"] }
async-stripe-checkout = { version = "1.0.0-rc.8", features = ["checkout_session"] }
async-stripe-billing = { version = "1.0.0-rc.8", features = ["subscription", "billing_portal_session"] }
async-stripe-product = { version = "1.0.0-rc.8", features = ["price"] }
async-stripe-webhook = { version = "1.0.0-rc.8", features = ["async-stripe-checkout"] }
async-stripe-types = "1.0.0-rc.8"
```

Não confirmei se `rustls-ring` é obrigatório junto com
`rustls-tls-webpki-roots` ou se um default de provider já vem embutido; a
página de features do docs.rs lista os dois como flags separadas, então o
seguro é declarar o provider explicitamente.

O exemplo oficial `examples/endpoints/Cargo.toml` usa exatamente esse
formato de features por recurso (edition 2024).

## 2. Cliente

Tipos: `stripe::Client`, `stripe::ClientBuilder`, `stripe::RequestStrategy`.

```rust
use std::time::Duration;
use stripe::{Client, ClientBuilder, RequestStrategy};

// simples
let client = Client::new(std::env::var("STRIPE_SECRET_KEY")?);

// com configuração
let client = ClientBuilder::new(secret_key)
    .app_info("quark", Some("1.0.0".to_string()), Some("https://example.com".to_string()))
    .request_strategy(RequestStrategy::Retry(3))
    .timeout(Duration::from_secs(15))   // timeout por tentativa
    .url("http://localhost:12111")      // útil para stripe-mock em testes
    .build()?;                          // -> Result<Client, StripeError>
```

Assinaturas verificadas do `ClientBuilder`: `new(secret: impl Into<String>)`,
`client_id(ApplicationId)`, `account_id(AccountId)` (header
`Stripe-Account`), `request_strategy(RequestStrategy)`,
`url(impl Into<String>)`, `timeout(Duration)`,
`app_info(name, Option<String>, Option<String>)`,
`build() -> Result<Client, StripeError>`.

Stripe-Version: **não há setter público**. O cliente sempre envia o header
`stripe-version` com a constante gerada
`stripe_shared::version::VERSION`, que na rc.8 é
`ApiVersion::V2026_07_29_dahlia` (ou seja, API `2026-07-29.dahlia`,
pinada pelo codegen). Verificado em
`async-stripe-client-core/src/config.rs` e
`generated/async-stripe-shared/src/version.rs` no master.

`RequestStrategy`: `Once` (default), `Idempotent(key)`, `Retry(n)`,
`ExponentialBackoff(n)`.

## 3. Criar Customer

O padrão da v1 é builder fluente que termina em `.send(&client).await`.
Snippet adaptado do exemplo oficial `examples/endpoints/src/checkout.rs`:

```rust
use stripe_core::customer::CreateCustomer;

let customer = CreateCustomer::new()
    .name("Alexander Lyon")
    .email("test@async-stripe.com")
    .metadata([(String::from("tenant_id"), String::from("t_123"))])
    .send(&client)
    .await?;
// customer.id: stripe_shared::CustomerId
```

## 4. Checkout Session em modo subscription

Tipos em `stripe_checkout::checkout_session`. Do exemplo oficial mais os
tipos de `subscription_data` verificados no docs.rs:

```rust
use stripe_checkout::CheckoutSessionMode;
use stripe_checkout::checkout_session::{
    CreateCheckoutSession, CreateCheckoutSessionLineItems,
    CreateCheckoutSessionSubscriptionData,
    CreateCheckoutSessionSubscriptionDataTrialSettings,
    CreateCheckoutSessionSubscriptionDataTrialSettingsEndBehavior,
    CreateCheckoutSessionSubscriptionDataTrialSettingsEndBehaviorMissingPaymentMethod,
};

let sub_data = CreateCheckoutSessionSubscriptionData {
    metadata: Some([(String::from("tenant_id"), String::from("t_123"))].into()),
    trial_period_days: Some(14), // Option<u32>
    trial_settings: Some(CreateCheckoutSessionSubscriptionDataTrialSettings::new(
        CreateCheckoutSessionSubscriptionDataTrialSettingsEndBehavior::new(
            CreateCheckoutSessionSubscriptionDataTrialSettingsEndBehaviorMissingPaymentMethod::Cancel,
        ),
    )),
    ..Default::default()
};

let session = CreateCheckoutSession::new()
    .mode(CheckoutSessionMode::Subscription)
    .customer(customer.id.as_str())
    .client_reference_id("t_123")
    .line_items(vec![CreateCheckoutSessionLineItems {
        price: Some(price.id.to_string()),
        quantity: Some(1),
        ..Default::default()
    }])
    .subscription_data(sub_data)
    .success_url("https://app.example.com/billing/ok?session_id={CHECKOUT_SESSION_ID}")
    .cancel_url("https://app.example.com/billing/cancel")
    .send(&client)
    .await?;
// session.url: Option<String> -> redirecionar o usuário para cá
```

Métodos verificados no builder: `mode(impl Into<CheckoutSessionMode>)`,
`customer(impl Into<String>)`,
`line_items(impl Into<Vec<CreateCheckoutSessionLineItems>>)`,
`client_reference_id(impl Into<String>)`, `success_url`, `cancel_url`,
`currency(impl Into<Currency>)`,
`locale(impl Into<CheckoutSessionLocale>)`,
`subscription_data(impl Into<CreateCheckoutSessionSubscriptionData>)`,
`metadata(impl Into<HashMap<String, String>>)`.

`CreateCheckoutSessionSubscriptionData` (struct de campos públicos com
`Default`): `metadata: Option<HashMap<String, String>>`,
`trial_period_days: Option<u32>`, `trial_end: Option<Timestamp>`,
`trial_settings: Option<...TrialSettings>`, `description`,
`proration_behavior`, `billing_cycle_anchor`, etc. O enum
`...MissingPaymentMethod` tem variantes `Cancel`, `CreateInvoice`, `Pause`
e `Unknown(String)` (non-exhaustive).

Nota: `currency` e `locale` não aparecem no exemplo oficial; as assinaturas
acima vêm da página do docs.rs de `CreateCheckoutSession`. Não confirmei as
variantes de `CheckoutSessionLocale` uma a uma.

## 5. Listar Price por lookup_key

`stripe_product::price::ListPrice` (feature `price`):

```rust
use stripe_product::price::ListPrice;

let prices = ListPrice::new()
    .lookup_keys(vec!["pro_monthly".to_string()]) // até 10 chaves
    .active(true)
    .send(&client)
    .await?;
// prices: stripe_types::List<stripe_shared::Price>
let price = prices.data.first();
// price.id: PriceId, price.lookup_key: Option<String>,
// price.unit_amount: Option<i64>, price.currency: Currency,
// price.recurring: Option<Recurring>, price.product: Expandable<Product>
```

Assinaturas verificadas:
`lookup_keys(self, impl Into<Vec<String>>) -> Self`,
`active(self, impl Into<bool>) -> Self`,
`type_(self, impl Into<PriceType>) -> Self`,
`send<C: StripeClient>(&self, &C) -> Result<Output, C::Err>`,
`paginate(&self) -> ListPaginator<List<Price>>`.

## 6. Billing Portal Session

`stripe_billing::billing_portal_session::CreateBillingPortalSession`
(feature `billing_portal_session`):

```rust
use stripe_billing::billing_portal_session::CreateBillingPortalSession;

let portal = CreateBillingPortalSession::new()
    .customer(customer_id.as_str())
    .return_url("https://app.example.com/settings/billing")
    .send(&client)
    .await?;
// portal.url: String
```

Assinaturas verificadas: `new()`, `customer(impl Into<String>)`,
`return_url(impl Into<String>)`,
`locale(impl Into<BillingPortalSessionLocale>)`,
`configuration(impl Into<String>)`,
`send(...) -> Result<BillingPortalSession, C::Err>`.
Nota: no docs.rs o `new()` aparece sem argumentos e `customer` como setter,
mas a API do Stripe exige customer; tratar como obrigatório na prática.

## 7. Retrieve Subscription

`stripe_billing::subscription::RetrieveSubscription` (feature
`subscription`):

```rust
use stripe_billing::subscription::RetrieveSubscription;
use stripe_shared::SubscriptionStatus;

let sub = RetrieveSubscription::new(sub_id) // impl Into<SubscriptionId>
    .expand(["items.data.price".to_string()])
    .send(&client)
    .await?;

match sub.status {
    SubscriptionStatus::Active | SubscriptionStatus::Trialing => { /* ok */ }
    SubscriptionStatus::PastDue => { /* inadimplente */ }
    _ => { /* sem acesso */ }
}
```

Campos relevantes de `stripe_shared::Subscription` (verificados):
`id: SubscriptionId`, `status: SubscriptionStatus`,
`customer: Expandable<Customer>`, `items: List<SubscriptionItem>`,
`metadata: HashMap<String, String>`, `cancel_at_period_end: bool`,
`trial_end: Option<Timestamp>`. **`current_period_end` não existe mais**
na struct (a API dahlia moveu período para o item); confirmar o campo
substituto em `SubscriptionItem` antes de depender dele, pois não verifiquei
os campos de `SubscriptionItem` um a um. O price do item carrega
`lookup_key: Option<String>` via `stripe_shared::Price`.

`SubscriptionStatus` (serializa em snake_case): `Active`, `Canceled`,
`Incomplete`, `IncompleteExpired`, `PastDue`, `Paused`, `Trialing`,
`Unpaid`, `Unknown(String)` (non-exhaustive).

## 8. Webhook

Crate `async-stripe-webhook`, lib `stripe_webhook`. A feature por área
habilita as variantes de `EventObject` daquela área (o exemplo oficial
`webhook-axum` habilita `async-stripe-checkout` para ter
`CheckoutSessionCompleted`). Para eventos de subscription/invoice, habilitar
também `async-stripe-billing` e/ou `async-stripe-core`; `full` liga tudo.

```rust
use stripe_webhook::{Event, EventObject, Webhook};

let event: Event = Webhook::construct_event(&payload, sig_header, "whsec_...")?;
// event.id, event.type_ (EventType), event.data (EventData)

match event.data.object {
    EventObject::CheckoutSessionCompleted(session) => { /* session: CheckoutSession */ }
    _ => println!("evento nao tratado: {:?}", event.type_),
}
```

Assinaturas verificadas na struct `Webhook`:

```rust
pub fn construct_event(payload: &str, sig: &str, secret: &str) -> Result<Event, WebhookError>;
pub fn construct_event_with_timestamp(payload: &str, sig: &str, secret: &str, timestamp: i64) -> Result<Event, WebhookError>;
pub fn generate_test_header(payload: &str, secret: &str, timestamp: Option<i64>) -> String; // gera o header stripe-signature para testes
pub fn insecure(payload: &str) -> Result<Event, WebhookError>; // sem verificar assinatura
```

`construct_event` rejeita timestamp com mais de 5 minutos; a variante
`_with_timestamp` existe justamente para replays em teste. Para testes,
`generate_test_header` produz o valor do header `stripe-signature` a partir
do payload e do secret. Não enumerei as variantes de `WebhookError` uma a
uma (a página verificada só descreve os modos de falha: assinatura
inválida, secret inválido, timestamp velho, parse do payload).

O match fino é em `event.data.object` (enum `EventObject`, uma variante por
tipo de evento, ex. `CheckoutSessionCompleted(CheckoutSession)`,
`AccountUpdated(Account)`); `event.type_` é o enum `EventType`
(`"checkout.session.completed"` etc.) para logging ou fallback.

## 9. Erros

`stripe::StripeError` (retornado por `send` e por `ClientBuilder::build`):

- `Stripe(Box<ApiErrors>, u16)`: o Stripe respondeu com erro (o `u16` é o
  status HTTP). `ApiErrors` traz `type_` (`ApiErrorsType`: api_error,
  card_error, idempotency_error, invalid_request_error) e `code`
  (`ApiErrorsCode`).
- `ClientError(String)`: falha de comunicação (rede).
- `Timeout`: requisição blocking estourou o timeout.
- `JSONDeserialize(String)`: resposta não parseou.
- `ConfigError(String)`: configuração inválida do client.

Distinção rede vs API: `matches!(err, StripeError::Stripe(..))` é erro da
API; `ClientError`/`Timeout` são rede.

## 10. Estabilidade e MSRV

- MSRV: `rust-version = "1.88.0"`, edition 2024 (Cargo.toml do workspace no
  master).
- API do Stripe pinada: `2026-07-29.dahlia`, sem override em runtime.
- O CHANGELOG.md do repositório **não tem entradas para a série rc** (só
  uma seção "1.0 Alpha" antiga e os releases 0.x). Não há promessa
  documentada de estabilidade entre rcs; sendo pre-release semver, breaking
  changes entre rc.N e rc.N+1 são possíveis (houve regeneração de bindings
  para novas versões da API do Stripe ao longo da série). Recomendação:
  pinar `=1.0.0-rc.8` no Cargo.toml.
- O README ainda cita `1.0.0-rc.5` no texto; a versão publicada mais nova é
  rc.8.

## O que não foi confirmado

- Variantes de `WebhookError` e de `CheckoutSessionLocale` uma a uma.
- Campos de `SubscriptionItem` (em particular onde ficou o
  `current_period_end` na API dahlia).
- Se `rustls-tls-webpki-roots` funciona sem declarar `rustls-ring` ou
  `rustls-aws-lc-rs` explicitamente.
- Notas de release por rc (o GitHub Releases não foi consultado release a
  release; o CHANGELOG não cobre a série).

## Fontes

- https://github.com/arlyon/async-stripe (README, branch master)
- https://github.com/arlyon/async-stripe/tree/master/examples (endpoints/src/checkout.rs, endpoints/src/subscriptions.rs, endpoints/src/client_config.rs, endpoints/Cargo.toml, webhook-axum/src/main.rs, webhook-axum/Cargo.toml)
- https://docs.rs/async-stripe/1.0.0-rc.8/stripe/ (Client, ClientBuilder, StripeError)
- https://docs.rs/async-stripe-checkout/1.0.0-rc.8/stripe_checkout/ (CreateCheckoutSession, CreateCheckoutSessionSubscriptionData e tipos de trial)
- https://docs.rs/async-stripe-billing/1.0.0-rc.8/stripe_billing/ (RetrieveSubscription, CreateBillingPortalSession)
- https://docs.rs/async-stripe-product/1.0.0-rc.8/stripe_product/price/struct.ListPrice.html
- https://docs.rs/async-stripe-webhook/1.0.0-rc.8/stripe_webhook/struct.Webhook.html
- https://docs.rs/async-stripe-shared/1.0.0-rc.8/stripe_shared/ (Subscription, SubscriptionStatus, Price)
- Páginas de features no docs.rs de async-stripe, async-stripe-billing e async-stripe-webhook (1.0.0-rc.8)
- https://raw.githubusercontent.com/arlyon/async-stripe/master/async-stripe-client-core/src/config.rs e generated/async-stripe-shared/src/version.rs (pin da Stripe-Version)
- https://crates.io/crates/async-stripe-webhook e https://lib.rs/crates/async-stripe
