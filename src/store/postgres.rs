use crate::analytics::{is_bot, Aggregates, AnalyticsSink, ClickEvent, Stats, EVENTS_MAX};
use crate::auth::ApiToken;
use crate::domain::{Domain, DomainStatus};
use crate::invite::Invite;
use crate::oidc::TenantOidcConfig;
use crate::pixel::{PixelConfig, PixelCredentials, Provider};
use crate::secretbox::{self, SecretBox};
use crate::sso::SsoEmailDomain;
use crate::store::{
    AlertRule, LinkHealth, OutboxDelivery, OutboxRow, Record, Store, StoreError, Variant,
};
use crate::tenant::{Membership, Role, Tenant, TenantId, User};
use crate::webhooks::{SubscriptionKind, WebhookSubscription};
use sqlx::postgres::{PgPoolOptions, PgRow};
use sqlx::{PgPool, Row};

/// AAD field label for the per-tenant OIDC `client_secret`. The full AAD is
/// `format!("{tenant_id}:{AAD_OIDC_CLIENT_SECRET}")`, binding the v2 ciphertext
/// to that row (LUC-62). Kept as a constant so seal and open never diverge.
const AAD_OIDC_CLIENT_SECRET: &str = "oidc_client_secret";

/// AAD field label for the Sheets `refresh_token`. See `AAD_OIDC_CLIENT_SECRET`.
const AAD_SHEETS_REFRESH_TOKEN: &str = "sheets_refresh_token";

/// Builds the OIDC `client_secret` AAD for a tenant: `<tenant_id>:oidc_client_secret`.
fn aad_oidc_client_secret(tenant: TenantId) -> Vec<u8> {
    format!("{}:{AAD_OIDC_CLIENT_SECRET}", tenant.0).into_bytes()
}

/// Builds the Sheets `refresh_token` AAD for a tenant: `<tenant_id>:sheets_refresh_token`.
fn aad_sheets_refresh_token(tenant: TenantId) -> Vec<u8> {
    format!("{}:{AAD_SHEETS_REFRESH_TOKEN}", tenant.0).into_bytes()
}

/// Routes a tenant-owned READ (or write-pool read) query through the correct
/// execution context. `$body` is an expression that uses the bound connection
/// `$c: &mut sqlx::PgConnection` and returns `Result<_, sqlx::Error>`.
///
/// - Cloud mode (`multi_tenant == true`): runs `$body` inside a per-tenant
///   transaction on the READ pool that first did `SET LOCAL app.tenant_id`
///   (so `FORCE ROW LEVEL SECURITY` is satisfied), then commits.
/// - OSS mode: runs `$body` directly on a pooled READ connection — no
///   transaction, no extra round-trip, byte-for-byte the pre-P2a path.
///
/// The macro evaluates to the (sqlx-error-mapped, `?`-propagated) value of
/// `$body`, so callers deserialize exactly as before.
macro_rules! with_read {
    ($self:expr, $tenant:expr, |$c:ident| $body:block) => {{
        if $self.multi_tenant {
            let mut tx = $self.begin_tenant_tx_read($tenant).await?;
            // The body runs in an immediately-awaited `async` block so an
            // intermediate `?` inside a multi-statement body propagates as
            // `sqlx::Error` (to here), not as `StoreError` (to the outer fn).
            let out = async {
                let $c: &mut sqlx::PgConnection = &mut *tx;
                $body
            }
            .await
            .map_err(StoreError::backend)?;
            tx.commit().await.map_err(StoreError::backend)?;
            out
        } else {
            let mut conn = $self.read.acquire().await.map_err(StoreError::backend)?;
            async {
                let $c: &mut sqlx::PgConnection = &mut *conn;
                $body
            }
            .await
            .map_err(StoreError::backend)?
        }
    }};
}

/// Write-pool sibling of [`with_read!`]: same shape, but cloud mode uses the
/// WRITE-pool per-tenant transaction (`begin_tenant_tx`) and OSS mode a pooled
/// WRITE connection. Used for every tenant-owned mutation, plus the tenant reads
/// that must stay on the primary (read-your-writes), e.g. `get_sheets_connection`.
macro_rules! with_write {
    ($self:expr, $tenant:expr, |$c:ident| $body:block) => {{
        if $self.multi_tenant {
            let mut tx = $self.begin_tenant_tx($tenant).await?;
            let out = async {
                let $c: &mut sqlx::PgConnection = &mut *tx;
                $body
            }
            .await
            .map_err(StoreError::backend)?;
            tx.commit().await.map_err(StoreError::backend)?;
            out
        } else {
            let mut conn = $self.write.acquire().await.map_err(StoreError::backend)?;
            async {
                let $c: &mut sqlx::PgConnection = &mut *conn;
                $body
            }
            .await
            .map_err(StoreError::backend)?
        }
    }};
}

/// Key of the pg_advisory_lock that serializes idempotent schema creation across instances.
const QUARK_SCHEMA_LOCK_ID: i64 = 727271;

/// Visibility lease (seconds) applied by `claim_due_deliveries`: a claimed row
/// has its `next_attempt_at` pushed this far out so a concurrent relay skips
/// it, while a relay that crashes mid-delivery has the row re-claimed once the
/// lease expires (at-least-once). Comfortably longer than one delivery attempt.
const CLAIM_LEASE_SECS: u64 = 60;

/// Every tenant-owned table: gets a `tenant_id` column and an RLS policy.
/// (Global counters/sequences and lease tables are intentionally absent.)
const TENANT_OWNED_TABLES: [&str; 18] = [
    "links",
    "aliases",
    "alert_rules",
    "link_health",
    "sessions",
    "webhooks",
    "api_tokens",
    "pixels",
    "wellknown_documents",
    "click_counters",
    "stats_meta",
    "click_events",
    "webhook_deliveries",
    "sheets_connection",
    "domains",
    "invites",
    "oidc_configs",
    "sso_email_domains",
];

/// Maps a Postgres unique-constraint violation (SQLSTATE 23505) to
/// `StoreError::UniqueViolation`, anything else to `StoreError::Backend`.
/// Shared by every `put_*` whose target has a UNIQUE column the caller wants
/// to turn into a 409 instead of a 503 (`put_tenant`'s `slug`, `put_domain`'s
/// `host`).
fn map_unique_violation(e: sqlx::Error) -> StoreError {
    if let sqlx::Error::Database(dbe) = &e {
        if dbe.code().as_deref() == Some("23505") {
            return StoreError::UniqueViolation;
        }
    }
    StoreError::backend(e)
}

/// Escapes `LIKE`/`ILIKE` wildcards (default escape char = `\`) so that the
/// user's term is treated literally. Order matters: escape `\` first.
fn like_escape(q: &str) -> String {
    q.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Maps a `links` row (id, url, expiry, created, tags, max_visits, rules,
/// variants, tenant_id) into `(id, Record)`.
/// Shared by `get_link`, `list_links`, and `search_links`, which select the
/// same columns.
fn row_to_link(r: &PgRow) -> Result<(u64, Record), StoreError> {
    let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
    let url: String = r.try_get("url").map_err(StoreError::backend)?;
    let expiry: Option<i64> = r.try_get("expiry").map_err(StoreError::backend)?;
    let created: i64 = r.try_get("created").map_err(StoreError::backend)?;
    let tags: serde_json::Value = r.try_get("tags").map_err(StoreError::backend)?;
    let tags: Vec<String> = serde_json::from_value(tags)?;
    let max_visits: Option<i64> = r.try_get("max_visits").map_err(StoreError::backend)?;
    let rules: serde_json::Value = r.try_get("rules").map_err(StoreError::backend)?;
    let rules: Vec<crate::store::Rule> = serde_json::from_value(rules)?;
    let variants: serde_json::Value = r.try_get("variants").map_err(StoreError::backend)?;
    let variants: Vec<Variant> = serde_json::from_value(variants)?;
    let app_ios: Option<String> = r.try_get("app_ios").map_err(StoreError::backend)?;
    let app_android: Option<String> = r.try_get("app_android").map_err(StoreError::backend)?;
    let folder: Option<String> = r.try_get("folder").map_err(StoreError::backend)?;
    let fallback_url: Option<String> = r.try_get("fallback_url").map_err(StoreError::backend)?;
    let password_hash: Option<String> = r.try_get("password_hash").map_err(StoreError::backend)?;
    let tenant_id: i64 = r.try_get("tenant_id").map_err(StoreError::backend)?;
    Ok((
        id as u64,
        Record {
            url,
            expiry: expiry.map(|v| v as u64),
            created: created as u64,
            tags,
            max_visits: max_visits.map(|v| v as u32),
            rules,
            variants,
            app_ios,
            app_android,
            folder,
            fallback_url,
            password_hash,
            tenant_id: TenantId(tenant_id as u64),
        },
    ))
}

/// Maps an `invites` row into an `Invite`.
fn row_to_invite(r: &PgRow) -> Result<Invite, StoreError> {
    let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
    let tenant_id: i64 = r.try_get("tenant_id").map_err(StoreError::backend)?;
    let email: String = r.try_get("email").map_err(StoreError::backend)?;
    let role: String = r.try_get("role").map_err(StoreError::backend)?;
    let token_hash: String = r.try_get("token_hash").map_err(StoreError::backend)?;
    let invited_by: i64 = r.try_get("invited_by").map_err(StoreError::backend)?;
    let created: i64 = r.try_get("created").map_err(StoreError::backend)?;
    let expires: i64 = r.try_get("expires").map_err(StoreError::backend)?;
    let accepted_at: Option<i64> = r.try_get("accepted_at").map_err(StoreError::backend)?;
    let accepted_by: Option<i64> = r.try_get("accepted_by").map_err(StoreError::backend)?;
    Ok(Invite {
        id: id as u64,
        tenant_id: TenantId(tenant_id as u64),
        email,
        role: role_from_str(&role)?,
        token_hash,
        invited_by: invited_by as u64,
        created: created as u64,
        expires: expires as u64,
        accepted_at: accepted_at.map(|v| v as u64),
        accepted_by: accepted_by.map(|v| v as u64),
    })
}

/// Maps a `domains` row into a `Domain`.
fn row_to_domain(r: &PgRow) -> Result<Domain, StoreError> {
    let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
    let tenant_id: i64 = r.try_get("tenant_id").map_err(StoreError::backend)?;
    let host: String = r.try_get("host").map_err(StoreError::backend)?;
    let token: String = r.try_get("token").map_err(StoreError::backend)?;
    let status: String = r.try_get("status").map_err(StoreError::backend)?;
    let created: i64 = r.try_get("created").map_err(StoreError::backend)?;
    let verified_at: Option<i64> = r.try_get("verified_at").map_err(StoreError::backend)?;
    let status = match status.as_str() {
        "verified" => DomainStatus::Verified,
        _ => DomainStatus::Pending,
    };
    Ok(Domain {
        id: id as u64,
        tenant_id: crate::tenant::TenantId(tenant_id as u64),
        host,
        token,
        status,
        created: created as u64,
        verified_at: verified_at.map(|v| v as u64),
    })
}

/// Maps an `sso_email_domains` row into an `SsoEmailDomain`.
fn row_to_sso_domain(r: &PgRow) -> Result<SsoEmailDomain, StoreError> {
    let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
    let tenant_id: i64 = r.try_get("tenant_id").map_err(StoreError::backend)?;
    let domain: String = r.try_get("domain").map_err(StoreError::backend)?;
    let token: String = r.try_get("token").map_err(StoreError::backend)?;
    let status: String = r.try_get("status").map_err(StoreError::backend)?;
    let created: i64 = r.try_get("created").map_err(StoreError::backend)?;
    let verified_at: Option<i64> = r.try_get("verified_at").map_err(StoreError::backend)?;
    let status = match status.as_str() {
        "verified" => DomainStatus::Verified,
        _ => DomainStatus::Pending,
    };
    Ok(SsoEmailDomain {
        id: id as u64,
        tenant_id: crate::tenant::TenantId(tenant_id as u64),
        domain,
        token,
        status,
        created: created as u64,
        verified_at: verified_at.map(|v| v as u64),
    })
}

/// The shape of `oidc_configs.blob`: every `TenantOidcConfig` field except the
/// two that ride as their own columns (`tenant_id`, `issuer`). `client_secret`
/// is sealed via `secretbox` when `QUARK_ENCRYPTION_KEY` is set (LUC-48),
/// plaintext otherwise.
#[derive(serde::Serialize, serde::Deserialize)]
struct OidcConfigBlob {
    client_id: String,
    client_secret: String,
    scopes: Vec<String>,
    admin_claim: String,
    admin_value: String,
    readonly_value: String,
    /// Group value mapping to `Role::Member` (write, no tenant admin).
    /// `#[serde(default)]` so a blob written before this field existed still
    /// deserializes (as an empty string, i.e. no member group).
    #[serde(default)]
    member_value: String,
    /// Optional required-group gate (multi-tenancy P2d Task 4b). `#[serde(default)]`
    /// so a blob written before this field existed still deserializes.
    #[serde(default)]
    required_value: Option<String>,
    post_login_url: Option<String>,
    /// Optional per-tenant post-logout redirect URI (RP-initiated logout,
    /// LUC-79). `#[serde(default)]` so a blob written before this field existed
    /// still deserializes.
    #[serde(default)]
    post_logout_url: Option<String>,
}

fn oidc_config_blob(cfg: &TenantOidcConfig, sb: &Option<SecretBox>) -> serde_json::Value {
    serde_json::to_value(OidcConfigBlob {
        client_id: cfg.client_id.clone(),
        client_secret: secretbox::seal_opt(
            sb,
            &cfg.client_secret,
            &aad_oidc_client_secret(cfg.tenant_id),
        ),
        scopes: cfg.scopes.clone(),
        admin_claim: cfg.admin_claim.clone(),
        admin_value: cfg.admin_value.clone(),
        readonly_value: cfg.readonly_value.clone(),
        member_value: cfg.member_value.clone(),
        required_value: cfg.required_value.clone(),
        post_login_url: cfg.post_login_url.clone(),
        post_logout_url: cfg.post_logout_url.clone(),
    })
    .expect("OidcConfigBlob has no non-serializable fields")
}

/// Maps an `oidc_configs` row (`tenant_id, issuer, blob`) into a
/// `TenantOidcConfig`. `sb` opens `client_secret` (encrypted-at-rest, opt-in
/// via `QUARK_ENCRYPTION_KEY`; legacy plaintext passes through unchanged).
fn row_to_oidc_config(r: &PgRow, sb: &Option<SecretBox>) -> Result<TenantOidcConfig, StoreError> {
    let tenant_id: i64 = r.try_get("tenant_id").map_err(StoreError::backend)?;
    let issuer: String = r.try_get("issuer").map_err(StoreError::backend)?;
    let blob: serde_json::Value = r.try_get("blob").map_err(StoreError::backend)?;
    let b: OidcConfigBlob = serde_json::from_value(blob)?;
    let tid = crate::tenant::TenantId(tenant_id as u64);
    let client_secret = secretbox::open_opt(sb, &b.client_secret, &aad_oidc_client_secret(tid))
        .map_err(StoreError::backend)?;
    Ok(TenantOidcConfig {
        tenant_id: tid,
        issuer,
        client_id: b.client_id,
        client_secret,
        scopes: b.scopes,
        admin_claim: b.admin_claim,
        admin_value: b.admin_value,
        readonly_value: b.readonly_value,
        member_value: b.member_value,
        required_value: b.required_value,
        post_login_url: b.post_login_url,
        post_logout_url: b.post_logout_url,
    })
}

/// True when `secret` should be re-sealed under the current primary key: it is
/// present, non-empty, and not already an `enc:v2:<primary_keyid>:` value. This
/// covers legacy plaintext, `enc:v1:`, and `enc:v2:` sealed under a rotated-out
/// (old) key. Used by `reencrypt_legacy_secrets` to skip rows already at the
/// target format. Idempotent by construction.
fn needs_reseal(secret: &Option<String>, primary_keyid: &str) -> bool {
    match secret {
        Some(s) => {
            if s.is_empty() {
                return false;
            }
            let already = format!("enc:v2:{primary_keyid}:");
            !s.starts_with(&already)
        }
        None => false,
    }
}

/// Maps a `webhooks` row into a `WebhookSubscription`.
fn row_to_webhook(r: &PgRow) -> Result<WebhookSubscription, StoreError> {
    let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
    let url: String = r.try_get("url").map_err(StoreError::backend)?;
    let events: serde_json::Value = r.try_get("events").map_err(StoreError::backend)?;
    let secret: String = r.try_get("secret").map_err(StoreError::backend)?;
    let active: bool = r.try_get("active").map_err(StoreError::backend)?;
    let created: i64 = r.try_get("created").map_err(StoreError::backend)?;
    let kind: String = r.try_get("kind").map_err(StoreError::backend)?;
    Ok(WebhookSubscription {
        id: id as u64,
        url,
        events: serde_json::from_value(events)?,
        secret,
        active,
        created: created as u64,
        kind: SubscriptionKind::from_str_or_generic(&kind),
    })
}

/// Maps a `webhook_deliveries` row (the columns `claim_due_deliveries`
/// returns) into an `OutboxDelivery`.
fn row_to_delivery(r: &PgRow) -> Result<OutboxDelivery, StoreError> {
    let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
    let delivery_key: String = r.try_get("delivery_key").map_err(StoreError::backend)?;
    let subscription_id: i64 = r.try_get("subscription_id").map_err(StoreError::backend)?;
    let event_type: String = r.try_get("event_type").map_err(StoreError::backend)?;
    let payload: String = r.try_get("payload").map_err(StoreError::backend)?;
    let attempts: i32 = r.try_get("attempts").map_err(StoreError::backend)?;
    let tenant_id: i64 = r.try_get("tenant_id").map_err(StoreError::backend)?;
    Ok(OutboxDelivery {
        id,
        delivery_key,
        subscription_id: subscription_id as u64,
        event_type,
        payload,
        attempts: attempts as u32,
        tenant_id: TenantId(tenant_id as u64),
    })
}

/// Maps an `api_tokens` row into an `ApiToken`.
fn row_to_api_token(r: &PgRow) -> Result<ApiToken, StoreError> {
    let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
    let name: String = r.try_get("name").map_err(StoreError::backend)?;
    let token_hash: String = r.try_get("token_hash").map_err(StoreError::backend)?;
    let scopes: serde_json::Value = r.try_get("scopes").map_err(StoreError::backend)?;
    let rate_limit_per_min: Option<i64> = r
        .try_get("rate_limit_per_min")
        .map_err(StoreError::backend)?;
    let created: i64 = r.try_get("created").map_err(StoreError::backend)?;
    let tenant_id: i64 = r.try_get("tenant_id").map_err(StoreError::backend)?;
    Ok(ApiToken {
        id: id as u64,
        name,
        token_hash,
        scopes: serde_json::from_value(scopes)?,
        rate_limit_per_min: rate_limit_per_min.map(|v| v as u32),
        created: created as u64,
        tenant_id: TenantId(tenant_id as u64),
    })
}

/// Maps a `Role` to/from the string stored in the `memberships.role` column.
fn role_to_str(r: Role) -> &'static str {
    match r {
        Role::Owner => "owner",
        Role::Admin => "admin",
        Role::Member => "member",
        Role::Viewer => "viewer",
    }
}
fn role_from_str(s: &str) -> Result<Role, StoreError> {
    match s {
        "owner" => Ok(Role::Owner),
        "admin" => Ok(Role::Admin),
        "member" => Ok(Role::Member),
        "viewer" => Ok(Role::Viewer),
        other => Err(StoreError::Backend(format!("unknown role: {other}"))),
    }
}

/// Maps a `memberships` row into a `Membership`.
fn row_to_membership(r: &PgRow) -> Result<Membership, StoreError> {
    let user_id: i64 = r.try_get("user_id").map_err(StoreError::backend)?;
    let tenant_id: i64 = r.try_get("tenant_id").map_err(StoreError::backend)?;
    let role: String = r.try_get("role").map_err(StoreError::backend)?;
    let created: i64 = r.try_get("created").map_err(StoreError::backend)?;
    Ok(Membership {
        user_id: user_id as u64,
        tenant_id: TenantId(tenant_id as u64),
        role: role_from_str(&role)?,
        created: created as u64,
    })
}

/// Maps a `Provider` to the string stored in the `pixels.provider` column.
fn provider_to_str(p: Provider) -> &'static str {
    match p {
        Provider::Ga4 => "ga4",
        Provider::MetaCapi => "meta_capi",
    }
}

/// Inverse of `provider_to_str`. Errors on an unrecognized value (defensive
/// against manual DB edits or a future provider not yet handled here).
fn provider_from_str(s: &str) -> Result<Provider, StoreError> {
    match s {
        "ga4" => Ok(Provider::Ga4),
        "meta_capi" => Ok(Provider::MetaCapi),
        other => Err(StoreError::Backend(format!(
            "unknown pixel provider: {other}"
        ))),
    }
}

/// Maps a `pixels` row into a `PixelConfig`.
fn row_to_pixel(r: &PgRow) -> Result<PixelConfig, StoreError> {
    let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
    let provider: String = r.try_get("provider").map_err(StoreError::backend)?;
    let credentials: serde_json::Value = r.try_get("credentials").map_err(StoreError::backend)?;
    let active: bool = r.try_get("active").map_err(StoreError::backend)?;
    let created: i64 = r.try_get("created").map_err(StoreError::backend)?;
    Ok(PixelConfig {
        id: id as u64,
        provider: provider_from_str(&provider)?,
        credentials: serde_json::from_value::<PixelCredentials>(credentials)?,
        active,
        created: created as u64,
    })
}

/// Maps an `alert_rules` row (`threshold`, `window_secs`) into an `AlertRule`.
fn row_to_alert_rule(r: &PgRow) -> AlertRule {
    let threshold: i64 = r.get("threshold");
    let window_secs: i64 = r.get("window_secs");
    AlertRule {
        threshold: threshold as u32,
        window_secs: window_secs as u64,
    }
}

/// Upserts a link row inside an open transaction (same SQL as `put_link`).
/// Shared by the `_tx` mutation methods so the link write and the outbox
/// enqueue commit atomically.
async fn upsert_link_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant: TenantId,
    id: u64,
    rec: &Record,
) -> Result<(), StoreError> {
    let tags = serde_json::to_value(&rec.tags)?;
    let rules = serde_json::to_value(&rec.rules)?;
    let variants = serde_json::to_value(&rec.variants)?;
    sqlx::query(
        "INSERT INTO links (id, url, expiry, created, tags, max_visits, rules, variants, app_ios, app_android, folder, fallback_url, password_hash, tenant_id) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14) \
         ON CONFLICT (id) DO UPDATE SET url=$2, expiry=$3, created=$4, tags=$5, max_visits=$6, rules=$7, variants=$8, app_ios=$9, app_android=$10, folder=$11, fallback_url=$12, password_hash=$13, tenant_id=$14",
    )
    .bind(id as i64)
    .bind(&rec.url)
    .bind(rec.expiry.map(|v| v as i64))
    .bind(rec.created as i64)
    .bind(&tags)
    .bind(rec.max_visits.map(|v| v as i64))
    .bind(&rules)
    .bind(&variants)
    .bind(&rec.app_ios)
    .bind(&rec.app_android)
    .bind(&rec.folder)
    .bind(&rec.fallback_url)
    .bind(&rec.password_hash)
    .bind(tenant.0 as i64)
    .execute(&mut **tx)
    .await
    .map_err(StoreError::backend)?;
    Ok(())
}

/// Enqueues the webhook-outbox `rows` inside an open transaction, with
/// `ON CONFLICT (delivery_key) DO NOTHING` (same idempotent insert as
/// `enqueue_deliveries`, but sharing the mutation's transaction). An empty
/// slice is a no-op (the LMDB case never reaches here).
async fn enqueue_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rows: &[OutboxRow],
) -> Result<(), StoreError> {
    for row in rows {
        sqlx::query(
            "INSERT INTO webhook_deliveries (delivery_key, subscription_id, event_type, payload, created, next_attempt_at, tenant_id) \
             VALUES ($1,$2,$3,$4,$5,$6,$7) ON CONFLICT (delivery_key) DO NOTHING",
        )
        .bind(&row.delivery_key)
        .bind(row.subscription_id as i64)
        .bind(&row.event_type)
        .bind(&row.payload)
        .bind(row.created as i64)
        .bind(row.next_attempt_at as i64)
        .bind(row.tenant_id.0 as i64)
        .execute(&mut **tx)
        .await
        .map_err(StoreError::backend)?;
    }
    Ok(())
}

pub struct PostgresStore {
    /// Primary pool: every write, plus the reads that must be read-your-writes
    /// fresh (auth). Was the single `pool` before the read/write split.
    write: PgPool,
    /// Read pool: the local read replica when `open_with_replica` is used, or a
    /// clone of `write` (the same handle) under single-URL `open`.
    read: PgPool,
    /// Whether this store was opened in multi-tenant (cloud) mode, from
    /// `QUARK_MULTI_TENANT`. In cloud mode `init_schema` forces RLS on the
    /// tenant-owned tables and every tenant-owned query runs inside a
    /// `SET LOCAL app.tenant_id` transaction (via `with_read!`/`with_write!`).
    /// In OSS mode this is false and queries run directly on the pool as before.
    multi_tenant: bool,
    /// Encryption-at-rest for secrets stored in this backend (OIDC
    /// `client_secret`, Sheets `refresh_token`), opt-in via
    /// `QUARK_ENCRYPTION_KEY`. `None` keeps every secret plaintext, byte-for-byte
    /// today's behavior.
    secretbox: Option<SecretBox>,
}

impl PostgresStore {
    pub async fn open(url: &str, multi_tenant: bool) -> Result<PostgresStore, StoreError> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(url)
            .await
            .map_err(StoreError::backend)?;
        // Single URL: both pools are the SAME handle (PgPool is an internal Arc,
        // so this clone is cheap and shares connections). Behavior is identical
        // to the pre-split single-pool store.
        let s = PostgresStore {
            read: pool.clone(),
            write: pool,
            multi_tenant,
            secretbox: SecretBox::from_env(),
        };
        s.init_schema().await?;
        Ok(s)
    }

    /// Opens a store with a separate read replica: `write` connects to the
    /// primary, `read` to the replica. Schema init/migration runs on `write`
    /// ONLY; the replica is read-only and receives the schema through
    /// streaming replication.
    pub async fn open_with_replica(
        primary_url: &str,
        replica_url: &str,
        multi_tenant: bool,
    ) -> Result<PostgresStore, StoreError> {
        let write = PgPoolOptions::new()
            .max_connections(10)
            .connect(primary_url)
            .await
            .map_err(StoreError::backend)?;
        let read = PgPoolOptions::new()
            .max_connections(10)
            .connect(replica_url)
            .await
            .map_err(StoreError::backend)?;
        let s = PostgresStore {
            write,
            read,
            multi_tenant,
            secretbox: SecretBox::from_env(),
        };
        s.init_schema().await?;
        Ok(s)
    }

    /// Whether this store is running in multi-tenant (cloud) mode. Plumbing
    /// only for now — nothing reads this yet.
    pub fn is_multi_tenant(&self) -> bool {
        self.multi_tenant
    }

    /// Creates the schema idempotently. `CREATE TABLE/SEQUENCE IF NOT EXISTS`
    /// can still collide under concurrency (several connections check "doesn't exist"
    /// and try to create at the same time, hitting the Postgres catalog's unique
    /// constraints) — so we serialize with a session advisory lock on a
    /// single connection before running the DDL.
    async fn init_schema(&self) -> Result<(), StoreError> {
        let mut conn = self.write.acquire().await.map_err(StoreError::backend)?;
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(QUARK_SCHEMA_LOCK_ID)
            .execute(&mut *conn)
            .await
            .map_err(StoreError::backend)?;

        let result = async {
            for ddl in [
                "CREATE SEQUENCE IF NOT EXISTS quark_id_seq",
                "CREATE SEQUENCE IF NOT EXISTS quark_webhook_id_seq",
                "CREATE SEQUENCE IF NOT EXISTS quark_api_token_id_seq",
                "CREATE TABLE IF NOT EXISTS links (id BIGINT PRIMARY KEY, url TEXT NOT NULL, expiry BIGINT, created BIGINT NOT NULL, tags JSONB NOT NULL DEFAULT '[]')",
                "ALTER TABLE links ADD COLUMN IF NOT EXISTS tags JSONB NOT NULL DEFAULT '[]'",
                "ALTER TABLE links ADD COLUMN IF NOT EXISTS rules JSONB NOT NULL DEFAULT '[]'",
                "ALTER TABLE links ADD COLUMN IF NOT EXISTS app_ios TEXT",
                "ALTER TABLE links ADD COLUMN IF NOT EXISTS app_android TEXT",
                "ALTER TABLE links ADD COLUMN IF NOT EXISTS folder TEXT",
                "ALTER TABLE links ADD COLUMN IF NOT EXISTS fallback_url TEXT",
                "ALTER TABLE links ADD COLUMN IF NOT EXISTS password_hash TEXT",
                "CREATE TABLE IF NOT EXISTS aliases (alias TEXT PRIMARY KEY, id BIGINT NOT NULL)",
                // P3 Task 2: alias namespace becomes per-domain instead of
                // global. Existing rows have no domain opinion, so they
                // default into the shared namespace (`SHARED_DOMAIN_ID` = 0).
                "ALTER TABLE aliases ADD COLUMN IF NOT EXISTS domain_id BIGINT NOT NULL DEFAULT 0",
                "CREATE TABLE IF NOT EXISTS link_health (id BIGINT PRIMARY KEY, checked_at BIGINT NOT NULL, status INT, healthy BOOLEAN NOT NULL)",
                "CREATE TABLE IF NOT EXISTS health_lease (id INT PRIMARY KEY, holder TEXT NOT NULL, expires_at BIGINT NOT NULL)",
                "CREATE TABLE IF NOT EXISTS sessions (token_hash TEXT PRIMARY KEY, subject TEXT NOT NULL, display TEXT NOT NULL, scopes JSONB NOT NULL, created BIGINT NOT NULL, expires BIGINT NOT NULL)",
                "CREATE TABLE IF NOT EXISTS stats (id BIGINT PRIMARY KEY, agg JSONB NOT NULL)",
                "CREATE TABLE IF NOT EXISTS events (id BIGINT PRIMARY KEY, recent JSONB NOT NULL)",
                "CREATE TABLE IF NOT EXISTS webhooks (id BIGINT PRIMARY KEY, url TEXT NOT NULL, events JSONB NOT NULL, secret TEXT NOT NULL, active BOOLEAN NOT NULL, created BIGINT NOT NULL, kind TEXT NOT NULL DEFAULT 'generic')",
                // `kind` (#6, native chat channels) is added after the fact for
                // deployments whose `webhooks` table predates it; pre-existing
                // rows have no kind opinion, so they default to `generic`
                // (same fallback `SubscriptionKind::from_str_or_generic` and
                // the LMDB/serde `#[serde(default)]` path use).
                "ALTER TABLE webhooks ADD COLUMN IF NOT EXISTS kind TEXT NOT NULL DEFAULT 'generic'",
                "CREATE TABLE IF NOT EXISTS api_tokens (id BIGINT PRIMARY KEY, name TEXT NOT NULL, token_hash TEXT NOT NULL, scopes JSONB NOT NULL, rate_limit_per_min BIGINT, created BIGINT NOT NULL)",
                "CREATE INDEX IF NOT EXISTS api_tokens_token_hash_idx ON api_tokens (token_hash)",
                // Primary link domain per tenant (LUC-86): the domain the copy
                // button and new links default to. NULL = fall back to the auto
                // subdomain, then the shared host.
                "ALTER TABLE tenants ADD COLUMN IF NOT EXISTS primary_domain_id BIGINT",
                // Idempotent migrations for pre-existing `links` tables (max-visits feature).
                "ALTER TABLE links ADD COLUMN IF NOT EXISTS max_visits BIGINT",
                "ALTER TABLE links ADD COLUMN IF NOT EXISTS visits BIGINT NOT NULL DEFAULT 0",
                "CREATE SEQUENCE IF NOT EXISTS quark_pixel_id_seq",
                "CREATE TABLE IF NOT EXISTS pixels (id BIGINT PRIMARY KEY, provider TEXT NOT NULL, credentials JSONB NOT NULL, active BOOLEAN NOT NULL, created BIGINT NOT NULL)",
                // Per-link click-threshold alert rules (LUC-38). Tenant-owned,
                // keyed by `(tenant_id, link_id)`.
                "CREATE TABLE IF NOT EXISTS alert_rules (tenant_id BIGINT NOT NULL DEFAULT 0, link_id BIGINT NOT NULL, threshold BIGINT NOT NULL, window_secs BIGINT NOT NULL, PRIMARY KEY (tenant_id, link_id))",
                // Idempotent migration for a `links` table created before variants
                // existed (#17).
                "ALTER TABLE links ADD COLUMN IF NOT EXISTS variants JSONB NOT NULL DEFAULT '[]'",
                "CREATE TABLE IF NOT EXISTS wellknown_documents (name TEXT NOT NULL, body TEXT NOT NULL, tenant_id BIGINT NOT NULL DEFAULT 0, PRIMARY KEY (tenant_id, name))",
                // Atomic analytics (scale-audit #4): counters incremented with
                // `ON CONFLICT DO UPDATE SET count = count + EXCLUDED.count`
                // instead of the old whole-blob read-modify-write under an
                // advisory lock. `stats`/`events` above are kept for
                // idempotency but no longer read or written.
                "CREATE TABLE IF NOT EXISTS click_counters (id BIGINT NOT NULL, dimension TEXT NOT NULL, bucket TEXT NOT NULL, count BIGINT NOT NULL, PRIMARY KEY (id, dimension, bucket))",
                "CREATE TABLE IF NOT EXISTS stats_meta (id BIGINT PRIMARY KEY, first_ts BIGINT NOT NULL, last_ts BIGINT NOT NULL)",
                "CREATE TABLE IF NOT EXISTS click_events (seq BIGSERIAL PRIMARY KEY, id BIGINT NOT NULL, ts BIGINT NOT NULL, referer TEXT, country TEXT, user_agent TEXT, city TEXT, variant INT, event_id TEXT NOT NULL DEFAULT '')",
                "CREATE INDEX IF NOT EXISTS click_events_id_seq_idx ON click_events (id, seq DESC)",
                // Durable webhook outbox (scale-audit #3): one row per (event,
                // subscription) delivery attempt-set. `delivery_key` UNIQUE
                // gives insert-time idempotency; the relay poll filters on
                // (dead, delivered_at, next_attempt_at), hence the index.
                "CREATE TABLE IF NOT EXISTS webhook_deliveries (id BIGSERIAL PRIMARY KEY, delivery_key TEXT UNIQUE NOT NULL, subscription_id BIGINT NOT NULL, event_type TEXT NOT NULL, payload TEXT NOT NULL, created BIGINT NOT NULL, attempts INT NOT NULL DEFAULT 0, next_attempt_at BIGINT NOT NULL, delivered_at BIGINT, dead BOOLEAN NOT NULL DEFAULT FALSE)",
                "CREATE INDEX IF NOT EXISTS webhook_deliveries_poll_idx ON webhook_deliveries (dead, delivered_at, next_attempt_at)",
                // Sheets connector (roadmap: Google Sheets). A single connection
                // row (single-tenant OSS), plus a lease mirroring `health_lease`
                // so only one node runs the scheduled sync at a time.
                // P1b Task 5: fresh DBs get the tenant-correct shape directly
                // (`tenant_id` PK, no `singleton`); the migration below reworks
                // any pre-existing table created under the old shape.
                "CREATE TABLE IF NOT EXISTS sheets_connection (tenant_id BIGINT NOT NULL DEFAULT 0 PRIMARY KEY, blob JSONB NOT NULL)",
                "CREATE TABLE IF NOT EXISTS sheets_lease (id INT PRIMARY KEY, holder TEXT NOT NULL, expires_at BIGINT NOT NULL)",
                // --- Multi-tenancy (P1a): identity tables + seeded default tenant ---
                "CREATE TABLE IF NOT EXISTS tenants (id BIGINT PRIMARY KEY, name TEXT NOT NULL, slug TEXT NOT NULL UNIQUE, created BIGINT NOT NULL)",
                "INSERT INTO tenants (id, name, slug, created) VALUES (0, 'default', 'default', 0) ON CONFLICT (id) DO NOTHING",
                "CREATE TABLE IF NOT EXISTS users (id BIGINT PRIMARY KEY, subject TEXT NOT NULL UNIQUE, email TEXT NOT NULL, display TEXT NOT NULL, created BIGINT NOT NULL)",
                "CREATE SEQUENCE IF NOT EXISTS quark_user_id_seq",
                // Starts at 1 so it never collides with the seeded default tenant (id 0).
                "CREATE SEQUENCE IF NOT EXISTS quark_tenant_id_seq START WITH 1",
                "CREATE TABLE IF NOT EXISTS memberships (user_id BIGINT NOT NULL, tenant_id BIGINT NOT NULL, role TEXT NOT NULL, created BIGINT NOT NULL, PRIMARY KEY (user_id, tenant_id))",
                "CREATE INDEX IF NOT EXISTS memberships_by_tenant ON memberships (tenant_id)",
                // --- Multi-tenancy (P1b): sessions carry the authenticated user ---
                "ALTER TABLE sessions ADD COLUMN IF NOT EXISTS user_id BIGINT NOT NULL DEFAULT 0",
                // --- RP-initiated logout (LUC-79): store the id_token for id_token_hint ---
                "ALTER TABLE sessions ADD COLUMN IF NOT EXISTS id_token TEXT",
                // --- Multi-tenancy (P3): custom domains ---
                // Starts at 1 so it never collides with the reserved shared-domain sentinel (0).
                "CREATE SEQUENCE IF NOT EXISTS quark_domain_id_seq START WITH 1",
                "CREATE TABLE IF NOT EXISTS domains (id BIGINT PRIMARY KEY, tenant_id BIGINT NOT NULL DEFAULT 0, host TEXT NOT NULL UNIQUE, token TEXT NOT NULL, status TEXT NOT NULL, created BIGINT NOT NULL, verified_at BIGINT)",
                // --- Multi-tenancy (P2c): team invites ---
                "CREATE SEQUENCE IF NOT EXISTS quark_invite_id_seq START WITH 1",
                "CREATE TABLE IF NOT EXISTS invites (id BIGINT PRIMARY KEY, tenant_id BIGINT NOT NULL DEFAULT 0, email TEXT NOT NULL, role TEXT NOT NULL, token_hash TEXT NOT NULL, invited_by BIGINT NOT NULL, created BIGINT NOT NULL, expires BIGINT NOT NULL, accepted_at BIGINT, accepted_by BIGINT)",
                "CREATE INDEX IF NOT EXISTS invites_token_hash_idx ON invites (token_hash)",
                // --- Multi-tenancy (P2d): per-tenant OIDC config ---
                "CREATE SEQUENCE IF NOT EXISTS quark_oidc_config_id_seq START WITH 1",
                "CREATE TABLE IF NOT EXISTS oidc_configs (id BIGINT PRIMARY KEY, tenant_id BIGINT NOT NULL DEFAULT 0, issuer TEXT NOT NULL, blob JSONB NOT NULL, created BIGINT NOT NULL)",
                "CREATE UNIQUE INDEX IF NOT EXISTS oidc_configs_tenant_idx ON oidc_configs (tenant_id)",
                // --- SSO email-domain discovery (LUC-57) ---
                // `domain` is UNIQUE across tenants: one tenant per email domain
                // (a verified domain routes discovery, so it can have one owner).
                "CREATE SEQUENCE IF NOT EXISTS quark_sso_domain_id_seq START WITH 1",
                "CREATE TABLE IF NOT EXISTS sso_email_domains (id BIGINT PRIMARY KEY, tenant_id BIGINT NOT NULL DEFAULT 0, domain TEXT NOT NULL UNIQUE, token TEXT NOT NULL, status TEXT NOT NULL, created BIGINT NOT NULL, verified_at BIGINT)",
            ] {
                sqlx::query(ddl)
                    .execute(&mut *conn)
                    .await
                    .map_err(StoreError::backend)?;
            }

            // A `tenant_id` column on every tenant-owned table (existing rows
            // default to 0 = the seeded default tenant). Idempotent via
            // `ADD COLUMN IF NOT EXISTS`.
            for table in TENANT_OWNED_TABLES {
                sqlx::query(&format!(
                    "ALTER TABLE {table} ADD COLUMN IF NOT EXISTS tenant_id BIGINT NOT NULL DEFAULT 0"
                ))
                .execute(&mut *conn)
                .await
                .map_err(StoreError::backend)?;
            }

            // P1b Task 5: tenant-correct primary keys, reworked from the
            // legacy single-tenant designs. Each block below is a true no-op
            // once the PK already covers the target column set: the `DO $$
            // ... $$` guard checks the *current* PK's columns via the catalog
            // (pg_index/pg_attribute) first, and only runs the drop/alter
            // when they don't already match. That keeps a table that's
            // already migrated (or was created fresh with the new shape
            // above) from having its PK index dropped and rebuilt (ACCESS
            // EXCLUSIVE lock) on every boot. Must run after the `tenant_id`
            // column backfill above, since both target PKs include
            // `tenant_id`.
            //
            // sheets_connection: drop the legacy `singleton` PK/column, key on
            // `tenant_id` alone. The old `sheets_connection_by_tenant` unique
            // index is now redundant with the PK and is dropped.
            for ddl in [
                "DO $$ \
                BEGIN \
                  IF NOT EXISTS ( \
                    SELECT 1 \
                    FROM pg_index i \
                    JOIN pg_class c ON c.oid = i.indrelid \
                    WHERE c.relname = 'sheets_connection' AND i.indisprimary \
                      AND ( \
                        SELECT array_agg(a.attname::text ORDER BY a.attname) \
                        FROM pg_attribute a \
                        WHERE a.attrelid = c.oid AND a.attnum = ANY(i.indkey) \
                      ) = ARRAY['tenant_id'] \
                  ) THEN \
                    ALTER TABLE sheets_connection DROP CONSTRAINT IF EXISTS sheets_connection_pkey; \
                    ALTER TABLE sheets_connection DROP COLUMN IF EXISTS singleton; \
                    ALTER TABLE sheets_connection ADD PRIMARY KEY (tenant_id); \
                  END IF; \
                END $$",
                "DROP INDEX IF EXISTS sheets_connection_by_tenant",
                // wellknown_documents: PK was `name` alone, which cannot hold
                // two tenants' documents of the same name. Rework to
                // `(tenant_id, name)`. `array_agg(... ORDER BY attname)`
                // sorts alphabetically, so the target is `['name',
                // 'tenant_id']`.
                "DO $$ \
                BEGIN \
                  IF NOT EXISTS ( \
                    SELECT 1 \
                    FROM pg_index i \
                    JOIN pg_class c ON c.oid = i.indrelid \
                    WHERE c.relname = 'wellknown_documents' AND i.indisprimary \
                      AND ( \
                        SELECT array_agg(a.attname::text ORDER BY a.attname) \
                        FROM pg_attribute a \
                        WHERE a.attrelid = c.oid AND a.attnum = ANY(i.indkey) \
                      ) = ARRAY['name', 'tenant_id'] \
                  ) THEN \
                    ALTER TABLE wellknown_documents DROP CONSTRAINT IF EXISTS wellknown_documents_pkey; \
                    ALTER TABLE wellknown_documents ADD PRIMARY KEY (tenant_id, name); \
                  END IF; \
                END $$",
                // aliases: PK was `alias` alone (a single global namespace).
                // Rework to `(domain_id, alias)` so the same alias string can
                // exist once per domain (P3 Task 2). `array_agg(... ORDER BY
                // attname)` sorts alphabetically, so the target is
                // `['alias', 'domain_id']`.
                "DO $$ \
                BEGIN \
                  IF NOT EXISTS ( \
                    SELECT 1 \
                    FROM pg_index i \
                    JOIN pg_class c ON c.oid = i.indrelid \
                    WHERE c.relname = 'aliases' AND i.indisprimary \
                      AND ( \
                        SELECT array_agg(a.attname::text ORDER BY a.attname) \
                        FROM pg_attribute a \
                        WHERE a.attrelid = c.oid AND a.attnum = ANY(i.indkey) \
                      ) = ARRAY['alias', 'domain_id'] \
                  ) THEN \
                    ALTER TABLE aliases DROP CONSTRAINT IF EXISTS aliases_pkey; \
                    ALTER TABLE aliases ADD PRIMARY KEY (domain_id, alias); \
                  END IF; \
                END $$",
            ] {
                sqlx::query(ddl)
                    .execute(&mut *conn)
                    .await
                    .map_err(StoreError::backend)?;
            }

            // Per-tenant listing/aggregation indexes. Plain (non-CONCURRENTLY)
            // CREATE INDEX: the build takes a brief SHARE lock that blocks writes
            // for its duration — negligible here because these tables are small.
            // CONCURRENTLY was tried and rejected: `init_schema` runs on every
            // boot while holding a session advisory lock, and a CONCURRENTLY
            // build (which waits for concurrent transactions) under that lock
            // deadlocks when connections race the migration, and an interrupted
            // build leaves an INVALID index that `IF NOT EXISTS` then skips
            // forever. Non-blocking builds for genuinely large tables belong in a
            // dedicated out-of-band migration step, not this every-boot path.
            for ddl in [
                "CREATE INDEX IF NOT EXISTS links_by_tenant_id ON links (tenant_id, id)",
                "CREATE INDEX IF NOT EXISTS webhooks_by_tenant ON webhooks (tenant_id, id)",
                "CREATE INDEX IF NOT EXISTS pixels_by_tenant ON pixels (tenant_id, id)",
                "CREATE INDEX IF NOT EXISTS api_tokens_by_tenant ON api_tokens (tenant_id, id)",
                "CREATE INDEX IF NOT EXISTS click_counters_by_tenant ON click_counters (tenant_id, id, dimension, bucket)",
                "CREATE INDEX IF NOT EXISTS domains_by_tenant_id ON domains (tenant_id)",
                "CREATE INDEX IF NOT EXISTS invites_by_tenant ON invites (tenant_id, id)",
            ] {
                sqlx::query(ddl)
                    .execute(&mut *conn)
                    .await
                    .map_err(StoreError::backend)?;
            }

            // Row-level security is DEFINED here but not ENFORCED in P1a: the
            // table owner (the role that runs migrations and serves requests)
            // bypasses RLS because we never issue `FORCE ROW LEVEL SECURITY`.
            // The enforced isolation layer in P1a is the app-level
            // `WHERE tenant_id = $` predicate on every query. P1b flips FORCE on
            // (cloud mode) and drives `app.tenant_id` via `begin_tenant_tx`.
            for table in TENANT_OWNED_TABLES {
                sqlx::query(&format!("ALTER TABLE {table} ENABLE ROW LEVEL SECURITY"))
                    .execute(&mut *conn)
                    .await
                    .map_err(StoreError::backend)?;
                let policy = format!("{table}_tenant_isolation");
                // No `CREATE POLICY IF NOT EXISTS` exists; drop-then-create keeps
                // the idempotent-boot contract.
                sqlx::query(&format!("DROP POLICY IF EXISTS {policy} ON {table}"))
                    .execute(&mut *conn)
                    .await
                    .map_err(StoreError::backend)?;
                sqlx::query(&format!(
                    "CREATE POLICY {policy} ON {table} USING (tenant_id = current_setting('app.tenant_id', true)::bigint)"
                ))
                .execute(&mut *conn)
                .await
                .map_err(StoreError::backend)?;
            }

            // Cloud mode only: FORCE makes even the table owner (the role that
            // runs migrations and serves requests) obey the policy, so tenant
            // isolation no longer relies solely on the app-level `WHERE
            // tenant_id` predicate. `ALTER ... FORCE` is idempotent and
            // metadata-only (no table rewrite, no data lock beyond a brief
            // catalog update).
            //
            // Excepted: `api_tokens`, `sessions` — their hot-path lookups are BY
            // HASH (`get_api_token_by_hash`, `get_session_by_hash`), which must
            // find the row BEFORE the tenant is known — those run on the bare
            // pool with no `app.tenant_id` set, so FORCE would make them fail
            // closed and break authentication.
            //
            // Also excepted: `click_counters`, `stats_meta`, `click_events`
            // (analytics — `record_batch`/`stats`) and `webhook_deliveries`
            // (the cluster-wide outbox relay — `enqueue_deliveries`/
            // `claim_due_deliveries`/`mark_*`). Those accessors run on the bare
            // pool too and never `SET LOCAL app.tenant_id`, so FORCE would fail
            // their reads closed (0 rows) and their writes closed (WITH CHECK
            // rejects a row whose `tenant_id` doesn't match a session GUC that
            // was never set). Concretely: `put_link_tx`/`put_alias_and_link_tx`
            // insert into `webhook_deliveries` from inside the per-tenant tx —
            // that insert would see `app.tenant_id = N` but the delivery row
            // itself carries the *subscription's* tenant, so a mismatched
            // `WITH CHECK` would reject legitimate enqueues.
            //
            // Also excepted: `domains` (P3 custom domains). The public redirect
            // looks a domain up by `Host` before the tenant is known at all
            // (`get_domain_by_host`), on the bare pool with no `app.tenant_id`
            // set — FORCE would make that lookup fail closed and break every
            // custom-domain redirect.
            //
            // Also excepted: `aliases` (P3 Task 2). The alias namespace is now
            // scoped by `domain_id`, not by tenant: `get_alias` runs on the
            // bare pool the same way `get_domain_by_host` does, and the shared
            // namespace (`SHARED_DOMAIN_ID` = 0) is deliberately visible across
            // every tenant. `put_alias_and_link`/`put_alias_and_link_tx` still
            // write inside a per-tenant transaction and still stamp
            // `tenant_id` for bookkeeping, but isolation for reads is by
            // domain, so FORCE would fail every bare-pool lookup closed.
            //
            // Also excepted: `invites` (P2c team invites). The accept flow
            // only has the raw token before it knows which tenant the invite
            // belongs to, so `get_invite_by_hash`/`create_invite`/
            // `mark_invite_accepted` run on the bare pool with no
            // `app.tenant_id` set — FORCE would make that lookup fail closed
            // and break every invite acceptance.
            //
            // RLS stays merely ENABLED (not FORCED) on all of these; the app-level
            // `WHERE tenant_id`/`WHERE domain_id` predicate remains the
            // isolation layer for their scoped methods and for the bare-pool
            // accessors above.
            const NOT_FORCED: [&str; 11] = [
                "api_tokens",
                "sessions",
                "click_counters",
                "stats_meta",
                "click_events",
                "webhook_deliveries",
                "domains",
                "aliases",
                "invites",
                // Discovery resolves the tenant from the email domain on the
                // bare pool (`get_sso_domain_bare`) before any `app.tenant_id`
                // RLS context — same reasoning as `domains`/`invites` above.
                "sso_email_domains",
                // Login/callback resolves the tenant from the URL slug (via
                // `get_tenant_by_slug`) and reads the config on the bare pool
                // (`get_oidc_config_bare`) before there is any `app.tenant_id`
                // RLS context — same reasoning as `domains`/`invites` above.
                "oidc_configs",
            ];
            if self.multi_tenant {
                for table in TENANT_OWNED_TABLES
                    .iter()
                    .filter(|t| !NOT_FORCED.contains(t))
                {
                    sqlx::query(&format!("ALTER TABLE {table} FORCE ROW LEVEL SECURITY"))
                        .execute(&mut *conn)
                        .await
                        .map_err(StoreError::backend)?;
                }
            }

            // Multi-tenancy P4a Task 1: analytics rows created before the
            // `tenant_id` column was populated (or written by a version of
            // `record_batch` that didn't bind it yet) sit at the column
            // default, 0. Backfill them from the owning link now that the
            // link's `tenant_id` is known. Idempotent — the `tenant_id = 0`
            // guard means a re-run only touches rows still at the default,
            // so running this on every boot is a no-op once caught up.
            // Clicks whose link was since deleted have no match and stay 0
            // (orphaned, acceptable). No `CONCURRENTLY` (see the index note
            // above); these are plain, non-blocking `UPDATE`s under the same
            // advisory lock as the rest of this migration.
            let mut backfilled: i64 = 0;
            for ddl in [
                "UPDATE click_events ce SET tenant_id = l.tenant_id \
                 FROM links l WHERE l.id = ce.id AND ce.tenant_id = 0",
                "UPDATE click_counters cc SET tenant_id = l.tenant_id \
                 FROM links l WHERE l.id = cc.id AND cc.tenant_id = 0",
                "UPDATE stats_meta sm SET tenant_id = l.tenant_id \
                 FROM links l WHERE l.id = sm.id AND sm.tenant_id = 0",
            ] {
                let result = sqlx::query(ddl)
                    .execute(&mut *conn)
                    .await
                    .map_err(StoreError::backend)?;
                backfilled += result.rows_affected() as i64;
            }
            if backfilled > 0 {
                eprintln!(
                    "analytics tenant_id backfill: {backfilled} row(s) updated from links.tenant_id"
                );
            }

            Ok(())
        }
        .await;

        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(QUARK_SCHEMA_LOCK_ID)
            .execute(&mut *conn)
            .await
            .map_err(StoreError::backend)?;

        result
    }

    /// Used in tests: resets all state (and re-seeds the default tenant so the
    /// OSS/default-tenant path keeps working after a reset).
    pub async fn reset_for_tests(&self) -> Result<(), StoreError> {
        for q in [
            "TRUNCATE links, aliases, alert_rules, link_health, health_lease, sessions, stats, events, webhooks, api_tokens, pixels, wellknown_documents, click_counters, stats_meta, click_events, webhook_deliveries, sheets_connection, sheets_lease, tenants, users, memberships, domains, invites, oidc_configs, sso_email_domains RESTART IDENTITY",
            "ALTER SEQUENCE quark_id_seq RESTART WITH 1",
            "ALTER SEQUENCE quark_webhook_id_seq RESTART WITH 1",
            "ALTER SEQUENCE quark_api_token_id_seq RESTART WITH 1",
            "ALTER SEQUENCE quark_pixel_id_seq RESTART WITH 1",
            "ALTER SEQUENCE quark_user_id_seq RESTART WITH 1",
            "ALTER SEQUENCE quark_tenant_id_seq RESTART WITH 1",
            "ALTER SEQUENCE quark_domain_id_seq RESTART WITH 1",
            "ALTER SEQUENCE quark_invite_id_seq RESTART WITH 1",
            "ALTER SEQUENCE quark_oidc_config_id_seq RESTART WITH 1",
            "INSERT INTO tenants (id, name, slug, created) VALUES (0, 'default', 'default', 0) ON CONFLICT (id) DO NOTHING",
        ] {
            sqlx::query(q)
                .execute(&self.write)
                .await
                .map_err(StoreError::backend)?;
        }
        Ok(())
    }

    /// Cloud-mode WRITE transaction that sets `app.tenant_id` for RLS.
    /// `SET LOCAL` (via `set_config(..., true)`) scopes it to the transaction so
    /// a pooled connection never leaks the previous tenant. In P2a this drives
    /// `FORCE ROW LEVEL SECURITY`: every tenant-owned write runs inside one of
    /// these (via `with_write!` / `begin_write_tx`), and the app-level
    /// `WHERE tenant_id` predicate stays on as belt-and-suspenders.
    async fn begin_tenant_tx(
        &self,
        tenant: TenantId,
    ) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, StoreError> {
        let mut tx = self.write.begin().await.map_err(StoreError::backend)?;
        sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
            .bind(tenant.0.to_string())
            .execute(&mut *tx)
            .await
            .map_err(StoreError::backend)?;
        Ok(tx)
    }

    /// READ-pool mirror of [`begin_tenant_tx`]: a transaction on `self.read`
    /// that first sets `app.tenant_id`. Used by `with_read!` so tenant-owned
    /// reads/lists/aggregates run under RLS in cloud mode while still hitting
    /// the read replica (when one is configured).
    async fn begin_tenant_tx_read(
        &self,
        tenant: TenantId,
    ) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, StoreError> {
        let mut tx = self.read.begin().await.map_err(StoreError::backend)?;
        sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
            .bind(tenant.0.to_string())
            .execute(&mut *tx)
            .await
            .map_err(StoreError::backend)?;
        Ok(tx)
    }

    /// Begins the WRITE transaction used by the multi-statement mutation methods
    /// (`put_link`, `put_alias_and_link`, and the `*_tx` variants). In cloud
    /// mode it is a per-tenant tx (`SET LOCAL app.tenant_id`, satisfying FORCE
    /// RLS); in OSS mode it is a plain `write.begin()` — identical to pre-P2a.
    async fn begin_write_tx(
        &self,
        tenant: TenantId,
    ) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, StoreError> {
        if self.multi_tenant {
            self.begin_tenant_tx(tenant).await
        } else {
            self.write.begin().await.map_err(StoreError::backend)
        }
    }
}

#[async_trait::async_trait]
impl Store for PostgresStore {
    async fn next_id(&self, _tenant: TenantId) -> Result<u64, StoreError> {
        // Global id / short-code namespace (per Global Constraints).
        let row = sqlx::query("SELECT nextval('quark_id_seq') AS id")
            .fetch_one(&self.write)
            .await
            .map_err(StoreError::backend)?;
        let id: i64 = row.try_get("id").map_err(StoreError::backend)?;
        Ok(id as u64)
    }

    async fn get_link(&self, tenant: TenantId, id: u64) -> Result<Option<Record>, StoreError> {
        let row = with_read!(self, tenant, |c| {
            sqlx::query(
                "SELECT id, url, expiry, created, tags, max_visits, rules, variants, app_ios, app_android, folder, fallback_url, password_hash, tenant_id FROM links WHERE tenant_id = $1 AND id = $2",
            )
            .bind(tenant.0 as i64)
            .bind(id as i64)
            .fetch_optional(&mut *c)
            .await
        });
        match row {
            Some(r) => Ok(Some(row_to_link(&r)?.1)),
            None => Ok(None),
        }
    }

    async fn put_link(&self, tenant: TenantId, id: u64, rec: &Record) -> Result<(), StoreError> {
        let mut tx = self.begin_write_tx(tenant).await?;
        upsert_link_in_tx(&mut tx, tenant, id, rec).await?;
        tx.commit().await.map_err(StoreError::backend)?;
        Ok(())
    }

    async fn get_alias(&self, domain_id: u64, alias: &str) -> Result<Option<u64>, StoreError> {
        // Deliberately bare: no tenant transaction, no `app.tenant_id`. Alias
        // lookup is scoped by domain, not by tenant (`aliases` is in
        // `NOT_FORCED` for exactly this reason), mirroring
        // `get_domain_by_host`.
        let row = sqlx::query("SELECT id FROM aliases WHERE domain_id = $1 AND alias = $2")
            .bind(domain_id as i64)
            .bind(alias)
            .fetch_optional(&self.read)
            .await
            .map_err(StoreError::backend)?;
        match row {
            Some(r) => {
                let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
                Ok(Some(id as u64))
            }
            None => Ok(None),
        }
    }

    async fn put_alias_and_link(
        &self,
        tenant: TenantId,
        domain_id: u64,
        alias: &str,
        id: u64,
        rec: &Record,
    ) -> Result<bool, StoreError> {
        let mut tx = self.begin_write_tx(tenant).await?;
        let res = sqlx::query(
            "INSERT INTO aliases (alias, id, tenant_id, domain_id) VALUES ($1,$2,$3,$4) ON CONFLICT (domain_id, alias) DO NOTHING",
        )
        .bind(alias)
        .bind(id as i64)
        .bind(tenant.0 as i64)
        .bind(domain_id as i64)
        .execute(&mut *tx)
        .await
        .map_err(StoreError::backend)?;
        if res.rows_affected() == 0 {
            return Ok(false);
        }
        upsert_link_in_tx(&mut tx, tenant, id, rec).await?;
        tx.commit().await.map_err(StoreError::backend)?;
        Ok(true)
    }

    async fn put_link_tx(
        &self,
        tenant: TenantId,
        id: u64,
        rec: &Record,
        deliveries: &[OutboxRow],
    ) -> Result<(), StoreError> {
        let mut tx = self.begin_write_tx(tenant).await?;
        upsert_link_in_tx(&mut tx, tenant, id, rec).await?;
        enqueue_in_tx(&mut tx, deliveries).await?;
        tx.commit().await.map_err(StoreError::backend)?;
        Ok(())
    }

    async fn put_alias_and_link_tx(
        &self,
        tenant: TenantId,
        domain_id: u64,
        alias: &str,
        id: u64,
        rec: &Record,
        deliveries: &[OutboxRow],
    ) -> Result<bool, StoreError> {
        let mut tx = self.begin_write_tx(tenant).await?;
        let res = sqlx::query(
            "INSERT INTO aliases (alias, id, tenant_id, domain_id) VALUES ($1,$2,$3,$4) ON CONFLICT (domain_id, alias) DO NOTHING",
        )
        .bind(alias)
        .bind(id as i64)
        .bind(tenant.0 as i64)
        .bind(domain_id as i64)
        .execute(&mut *tx)
        .await
        .map_err(StoreError::backend)?;
        if res.rows_affected() == 0 {
            return Ok(false);
        }
        upsert_link_in_tx(&mut tx, tenant, id, rec).await?;
        enqueue_in_tx(&mut tx, deliveries).await?;
        tx.commit().await.map_err(StoreError::backend)?;
        Ok(true)
    }

    async fn delete_link_tx(
        &self,
        tenant: TenantId,
        id: u64,
        deliveries: &[OutboxRow],
    ) -> Result<(), StoreError> {
        let mut tx = self.begin_write_tx(tenant).await?;
        sqlx::query("DELETE FROM links WHERE tenant_id = $1 AND id = $2")
            .bind(tenant.0 as i64)
            .bind(id as i64)
            .execute(&mut *tx)
            .await
            .map_err(StoreError::backend)?;
        sqlx::query("DELETE FROM link_health WHERE tenant_id = $1 AND id = $2")
            .bind(tenant.0 as i64)
            .bind(id as i64)
            .execute(&mut *tx)
            .await
            .map_err(StoreError::backend)?;
        enqueue_in_tx(&mut tx, deliveries).await?;
        tx.commit().await.map_err(StoreError::backend)?;
        Ok(())
    }

    async fn list_links(
        &self,
        tenant: TenantId,
        after: Option<u64>,
        limit: usize,
        tag: Option<&str>,
        folder: Option<&str>,
        active_only: bool,
    ) -> Result<Vec<(u64, Record)>, StoreError> {
        let tag_json = tag.map(|t| serde_json::json!([t]));
        // Single WHERE clause (no N+1): when `active_only`, keep only links that
        // are unexpired and under their visit cap. `$6` toggles the predicate so
        // the default path is unchanged.
        let now = crate::now() as i64;
        let rows = with_read!(self, tenant, |c| {
            sqlx::query(
                "SELECT id, url, expiry, created, tags, max_visits, rules, variants, app_ios, app_android, folder, fallback_url, password_hash, tenant_id FROM links \
                 WHERE tenant_id = $5 \
                   AND ($1::bigint IS NULL OR id > $1) \
                   AND ($2::jsonb IS NULL OR tags @> $2) \
                   AND ($4::text IS NULL OR lower(folder) = lower($4)) \
                   AND (NOT $6 OR ((expiry IS NULL OR expiry > $7) AND (max_visits IS NULL OR visits < max_visits))) \
                 ORDER BY id LIMIT $3",
            )
            .bind(after.map(|a| a as i64))
            .bind(&tag_json)
            .bind(limit as i64)
            .bind(folder)
            .bind(tenant.0 as i64)
            .bind(active_only)
            .bind(now)
            .fetch_all(&mut *c)
            .await
        });
        rows.iter().map(row_to_link).collect()
    }

    #[allow(clippy::too_many_arguments)]
    async fn search_links(
        &self,
        tenant: TenantId,
        q: &str,
        after: Option<u64>,
        limit: usize,
        tag: Option<&str>,
        folder: Option<&str>,
        active_only: bool,
    ) -> Result<Vec<(u64, Record)>, StoreError> {
        let pattern = format!("%{}%", like_escape(q));
        let tag_json = tag.map(|t| serde_json::json!([t]));
        let now = crate::now() as i64;
        let rows = with_read!(self, tenant, |c| {
            sqlx::query(
                "SELECT DISTINCT l.id, l.url, l.expiry, l.created, l.tags, l.max_visits, l.rules, l.variants, l.app_ios, l.app_android, l.folder, l.fallback_url, l.password_hash, l.tenant_id \
                 FROM links l LEFT JOIN aliases a ON a.id = l.id AND a.tenant_id = l.tenant_id \
                 WHERE l.tenant_id = $6 \
                   AND ($1::bigint IS NULL OR l.id > $1) \
                   AND (l.url ILIKE $2 OR a.alias ILIKE $2) \
                   AND ($3::jsonb IS NULL OR l.tags @> $3) \
                   AND ($5::text IS NULL OR lower(l.folder) = lower($5)) \
                   AND (NOT $7 OR ((l.expiry IS NULL OR l.expiry > $8) AND (l.max_visits IS NULL OR l.visits < l.max_visits))) \
                 ORDER BY l.id LIMIT $4",
            )
            .bind(after.map(|a| a as i64))
            .bind(&pattern)
            .bind(&tag_json)
            .bind(limit as i64)
            .bind(folder)
            .bind(tenant.0 as i64)
            .bind(active_only)
            .bind(now)
            .fetch_all(&mut *c)
            .await
        });
        rows.iter().map(row_to_link).collect()
    }

    async fn list_aliases(&self, tenant: TenantId) -> Result<Vec<(String, u64)>, StoreError> {
        let rows = with_read!(self, tenant, |c| {
            sqlx::query("SELECT alias, id FROM aliases WHERE tenant_id = $1")
                .bind(tenant.0 as i64)
                .fetch_all(&mut *c)
                .await
        });
        let mut out = Vec::new();
        for r in rows {
            let alias: String = r.try_get("alias").map_err(StoreError::backend)?;
            let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
            out.push((alias, id as u64));
        }
        Ok(out)
    }

    async fn delete_link(&self, tenant: TenantId, id: u64) -> Result<(), StoreError> {
        with_write!(self, tenant, |c| {
            sqlx::query("DELETE FROM links WHERE tenant_id = $1 AND id = $2")
                .bind(tenant.0 as i64)
                .bind(id as i64)
                .execute(&mut *c)
                .await?;
            sqlx::query("DELETE FROM link_health WHERE tenant_id = $1 AND id = $2")
                .bind(tenant.0 as i64)
                .bind(id as i64)
                .execute(&mut *c)
                .await
        });
        Ok(())
    }

    async fn delete_alias(&self, tenant: TenantId, alias: &str) -> Result<(), StoreError> {
        with_write!(self, tenant, |c| {
            sqlx::query("DELETE FROM aliases WHERE tenant_id = $1 AND alias = $2")
                .bind(tenant.0 as i64)
                .bind(alias)
                .execute(&mut *c)
                .await
        });
        Ok(())
    }

    async fn list_webhooks(
        &self,
        tenant: TenantId,
    ) -> Result<Vec<WebhookSubscription>, StoreError> {
        let rows = with_read!(self, tenant, |c| {
            sqlx::query(
                "SELECT id, url, events, secret, active, created, kind FROM webhooks WHERE tenant_id = $1 ORDER BY id",
            )
            .bind(tenant.0 as i64)
            .fetch_all(&mut *c)
            .await
        });
        rows.iter().map(row_to_webhook).collect()
    }

    async fn get_webhook(
        &self,
        tenant: TenantId,
        id: u64,
    ) -> Result<Option<WebhookSubscription>, StoreError> {
        let row = with_read!(self, tenant, |c| {
            sqlx::query(
                "SELECT id, url, events, secret, active, created, kind FROM webhooks WHERE tenant_id = $1 AND id = $2",
            )
            .bind(tenant.0 as i64)
            .bind(id as i64)
            .fetch_optional(&mut *c)
            .await
        });
        match row {
            Some(r) => Ok(Some(row_to_webhook(&r)?)),
            None => Ok(None),
        }
    }

    async fn put_webhook(
        &self,
        tenant: TenantId,
        sub: &WebhookSubscription,
    ) -> Result<(), StoreError> {
        let events = serde_json::to_value(&sub.events)?;
        with_write!(self, tenant, |c| {
            sqlx::query(
                "INSERT INTO webhooks (id, url, events, secret, active, created, kind, tenant_id) VALUES ($1,$2,$3,$4,$5,$6,$7,$8) \
                 ON CONFLICT (id) DO UPDATE SET url=$2, events=$3, secret=$4, active=$5, created=$6, kind=$7, tenant_id=$8",
            )
            .bind(sub.id as i64)
            .bind(&sub.url)
            .bind(&events)
            .bind(&sub.secret)
            .bind(sub.active)
            .bind(sub.created as i64)
            .bind(sub.kind.as_str())
            .bind(tenant.0 as i64)
            .execute(&mut *c)
            .await
        });
        Ok(())
    }

    async fn delete_webhook(&self, tenant: TenantId, id: u64) -> Result<bool, StoreError> {
        let res = with_write!(self, tenant, |c| {
            sqlx::query("DELETE FROM webhooks WHERE tenant_id = $1 AND id = $2")
                .bind(tenant.0 as i64)
                .bind(id as i64)
                .execute(&mut *c)
                .await
        });
        Ok(res.rows_affected() > 0)
    }

    async fn next_webhook_id(&self, _tenant: TenantId) -> Result<u64, StoreError> {
        // Global id namespace.
        let row = sqlx::query("SELECT nextval('quark_webhook_id_seq') AS id")
            .fetch_one(&self.write)
            .await
            .map_err(StoreError::backend)?;
        let id: i64 = row.try_get("id").map_err(StoreError::backend)?;
        Ok(id as u64)
    }

    async fn list_tags(&self, tenant: TenantId) -> Result<Vec<(String, u64)>, StoreError> {
        // Dedupe tags within a link (SELECT DISTINCT per id) before counting,
        // so a link carrying the same tag twice still counts once.
        let rows = with_read!(self, tenant, |c| {
            sqlx::query(
                "SELECT tag, count(*) AS n FROM ( \
                   SELECT DISTINCT id, jsonb_array_elements_text(tags) AS tag FROM links WHERE tenant_id = $1 \
                 ) t GROUP BY tag ORDER BY tag",
            )
            .bind(tenant.0 as i64)
            .fetch_all(&mut *c)
            .await
        });
        rows.iter()
            .map(|r| {
                let name: String = r.try_get("tag").map_err(StoreError::backend)?;
                let n: i64 = r.try_get("n").map_err(StoreError::backend)?;
                Ok((name, n as u64))
            })
            .collect()
    }

    async fn list_folders(&self, tenant: TenantId) -> Result<Vec<(String, u64)>, StoreError> {
        let rows = with_read!(self, tenant, |c| {
            sqlx::query(
                "SELECT folder, count(*) AS n FROM links WHERE tenant_id = $1 AND folder IS NOT NULL GROUP BY folder ORDER BY folder",
            )
            .bind(tenant.0 as i64)
            .fetch_all(&mut *c)
            .await
        });
        rows.iter()
            .map(|r| {
                let name: String = r.try_get("folder").map_err(StoreError::backend)?;
                let n: i64 = r.try_get("n").map_err(StoreError::backend)?;
                Ok((name, n as u64))
            })
            .collect()
    }

    async fn list_api_tokens(&self, tenant: TenantId) -> Result<Vec<ApiToken>, StoreError> {
        let rows = with_read!(self, tenant, |c| {
            sqlx::query(
                "SELECT id, name, token_hash, scopes, rate_limit_per_min, created, tenant_id \
                 FROM api_tokens WHERE tenant_id = $1 ORDER BY id",
            )
            .bind(tenant.0 as i64)
            .fetch_all(&mut *c)
            .await
        });
        rows.iter().map(row_to_api_token).collect()
    }

    /// Hot path of auth: indexed lookup by `token_hash`
    /// (`api_tokens_token_hash_idx`).
    async fn get_api_token_by_hash(&self, hash: &str) -> Result<Option<ApiToken>, StoreError> {
        let row = sqlx::query(
            "SELECT id, name, token_hash, scopes, rate_limit_per_min, created, tenant_id \
             FROM api_tokens WHERE token_hash = $1",
        )
        .bind(hash)
        .fetch_optional(&self.write)
        .await
        .map_err(StoreError::backend)?;
        match row {
            Some(r) => Ok(Some(row_to_api_token(&r)?)),
            None => Ok(None),
        }
    }

    async fn put_api_token(&self, tenant: TenantId, token: &ApiToken) -> Result<(), StoreError> {
        let scopes = serde_json::to_value(&token.scopes)?;
        with_write!(self, tenant, |c| {
            sqlx::query(
                "INSERT INTO api_tokens (id, name, token_hash, scopes, rate_limit_per_min, created, tenant_id) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7) \
                 ON CONFLICT (id) DO UPDATE SET \
                 name=$2, token_hash=$3, scopes=$4, rate_limit_per_min=$5, created=$6, tenant_id=$7",
            )
            .bind(token.id as i64)
            .bind(&token.name)
            .bind(&token.token_hash)
            .bind(&scopes)
            .bind(token.rate_limit_per_min.map(|v| v as i64))
            .bind(token.created as i64)
            .bind(tenant.0 as i64)
            .execute(&mut *c)
            .await
        });
        Ok(())
    }

    async fn delete_api_token(&self, tenant: TenantId, id: u64) -> Result<bool, StoreError> {
        let res = with_write!(self, tenant, |c| {
            sqlx::query("DELETE FROM api_tokens WHERE tenant_id = $1 AND id = $2")
                .bind(tenant.0 as i64)
                .bind(id as i64)
                .execute(&mut *c)
                .await
        });
        Ok(res.rows_affected() > 0)
    }

    async fn next_api_token_id(&self, _tenant: TenantId) -> Result<u64, StoreError> {
        // Global id namespace.
        let row = sqlx::query("SELECT nextval('quark_api_token_id_seq') AS id")
            .fetch_one(&self.write)
            .await
            .map_err(StoreError::backend)?;
        let id: i64 = row.try_get("id").map_err(StoreError::backend)?;
        Ok(id as u64)
    }

    async fn bump_visits(&self, tenant: TenantId, id: u64) -> Result<u64, StoreError> {
        let row = with_write!(self, tenant, |c| {
            sqlx::query(
                "UPDATE links SET visits = visits + 1 WHERE tenant_id = $1 AND id = $2 RETURNING visits",
            )
            .bind(tenant.0 as i64)
            .bind(id as i64)
            .fetch_one(&mut *c)
            .await
        });
        let visits: i64 = row.try_get("visits").map_err(StoreError::backend)?;
        Ok(visits as u64)
    }

    async fn visits(&self, tenant: TenantId, id: u64) -> Result<u64, StoreError> {
        let row = with_read!(self, tenant, |c| {
            sqlx::query("SELECT visits FROM links WHERE tenant_id = $1 AND id = $2")
                .bind(tenant.0 as i64)
                .bind(id as i64)
                .fetch_optional(&mut *c)
                .await
        });
        match row {
            Some(r) => {
                let visits: i64 = r.try_get("visits").map_err(StoreError::backend)?;
                Ok(visits as u64)
            }
            None => Ok(0),
        }
    }

    async fn visits_for(
        &self,
        tenant: TenantId,
        ids: &[u64],
    ) -> Result<std::collections::HashMap<u64, u64>, StoreError> {
        if ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let id_list: Vec<i64> = ids.iter().map(|&i| i as i64).collect();
        let rows = with_read!(self, tenant, |c| {
            sqlx::query("SELECT id, visits FROM links WHERE tenant_id = $1 AND id = ANY($2)")
                .bind(tenant.0 as i64)
                .bind(&id_list)
                .fetch_all(&mut *c)
                .await
        });
        let mut out = std::collections::HashMap::with_capacity(rows.len());
        for r in &rows {
            let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
            let visits: i64 = r.try_get("visits").map_err(StoreError::backend)?;
            out.insert(id as u64, visits as u64);
        }
        Ok(out)
    }

    async fn put_link_health(
        &self,
        tenant: TenantId,
        id: u64,
        health: &LinkHealth,
    ) -> Result<(), StoreError> {
        with_write!(self, tenant, |c| {
            sqlx::query(
                "INSERT INTO link_health (id, checked_at, status, healthy, tenant_id) VALUES ($1,$2,$3,$4,$5) \
                 ON CONFLICT (id) DO UPDATE SET checked_at=$2, status=$3, healthy=$4, tenant_id=$5",
            )
            .bind(id as i64)
            .bind(health.checked_at as i64)
            .bind(health.status.map(|s| s as i32))
            .bind(health.healthy)
            .bind(tenant.0 as i64)
            .execute(&mut *c)
            .await
        });
        Ok(())
    }

    async fn list_link_health(
        &self,
        tenant: TenantId,
    ) -> Result<Vec<(u64, LinkHealth)>, StoreError> {
        let rows = with_read!(self, tenant, |c| {
            sqlx::query(
                "SELECT id, checked_at, status, healthy FROM link_health WHERE tenant_id = $1",
            )
            .bind(tenant.0 as i64)
            .fetch_all(&mut *c)
            .await
        });
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
            let checked_at: i64 = r.try_get("checked_at").map_err(StoreError::backend)?;
            let status: Option<i32> = r.try_get("status").map_err(StoreError::backend)?;
            let healthy: bool = r.try_get("healthy").map_err(StoreError::backend)?;
            out.push((
                id as u64,
                LinkHealth {
                    checked_at: checked_at as u64,
                    status: status.map(|s| s as u16),
                    healthy,
                },
            ));
        }
        Ok(out)
    }

    async fn put_session(
        &self,
        tenant: TenantId,
        session: &crate::auth::Session,
    ) -> Result<(), StoreError> {
        let scopes = serde_json::to_value(&session.scopes)?;
        // `sessions` is intentionally NOT `FORCE`d (its by-hash lookup runs
        // before the tenant is known), but this write still carries a tenant, so
        // it routes through the tenant-tx in cloud mode for consistency; the
        // `WHERE`/`tenant_id` column keeps isolation.
        with_write!(self, tenant, |c| {
            sqlx::query(
                "INSERT INTO sessions (token_hash, subject, display, scopes, created, expires, tenant_id, user_id, id_token) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) \
                 ON CONFLICT (token_hash) DO UPDATE \
                   SET subject=$2, display=$3, scopes=$4, created=$5, expires=$6, tenant_id=$7, user_id=$8, id_token=$9",
            )
            .bind(&session.token_hash)
            .bind(&session.subject)
            .bind(&session.display)
            .bind(&scopes)
            .bind(session.created as i64)
            .bind(session.expires as i64)
            .bind(tenant.0 as i64)
            .bind(session.user_id as i64)
            .bind(session.id_token.as_deref())
            .execute(&mut *c)
            .await
        });
        Ok(())
    }

    async fn get_session_by_hash(
        &self,
        token_hash: &str,
        now: u64,
    ) -> Result<Option<crate::auth::Session>, StoreError> {
        let row = sqlx::query(
            "SELECT token_hash, subject, display, scopes, created, expires, tenant_id, user_id, id_token FROM sessions \
             WHERE token_hash = $1 AND expires > $2",
        )
        .bind(token_hash)
        .bind(now as i64)
        .fetch_optional(&self.write)
        .await
        .map_err(StoreError::backend)?;
        match row {
            Some(r) => {
                let scopes: serde_json::Value = r.try_get("scopes").map_err(StoreError::backend)?;
                let created: i64 = r.try_get("created").map_err(StoreError::backend)?;
                let expires: i64 = r.try_get("expires").map_err(StoreError::backend)?;
                let tenant_id: i64 = r.try_get("tenant_id").map_err(StoreError::backend)?;
                let user_id: i64 = r.try_get("user_id").map_err(StoreError::backend)?;
                Ok(Some(crate::auth::Session {
                    token_hash: r.try_get("token_hash").map_err(StoreError::backend)?,
                    subject: r.try_get("subject").map_err(StoreError::backend)?,
                    display: r.try_get("display").map_err(StoreError::backend)?,
                    scopes: serde_json::from_value(scopes)?,
                    created: created as u64,
                    expires: expires as u64,
                    tenant_id: TenantId(tenant_id as u64),
                    user_id: user_id as u64,
                    id_token: r.try_get("id_token").ok().flatten(),
                }))
            }
            None => Ok(None),
        }
    }

    async fn delete_session(&self, token_hash: &str) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(token_hash)
            .execute(&self.write)
            .await
            .map_err(StoreError::backend)?;
        Ok(())
    }

    async fn gc_sessions(&self, now: u64) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM sessions WHERE expires <= $1")
            .bind(now as i64)
            .execute(&self.write)
            .await
            .map_err(StoreError::backend)?;
        Ok(())
    }

    async fn purge_click_events_before(&self, cutoff_ts: u64) -> Result<u64, StoreError> {
        // Global purge: retention applies across every tenant, so no tenant
        // scope. Only the near-PII detail (`click_events`) is deleted; the
        // per-link aggregates (`click_counters`/`stats_meta`) are kept.
        let res = sqlx::query("DELETE FROM click_events WHERE ts < $1")
            .bind(cutoff_ts as i64)
            .execute(&self.write)
            .await
            .map_err(StoreError::backend)?;
        Ok(res.rows_affected())
    }

    async fn delete_link_analytics(&self, tenant: TenantId, id: u64) -> Result<(), StoreError> {
        // One transaction (per-tenant in cloud mode) so a link's detail,
        // counters and stats are erased atomically. Every delete is also
        // scoped by `tenant_id` (belt-and-suspenders, since these tables are
        // NOT_FORCED for RLS).
        let mut tx = self.begin_write_tx(tenant).await?;
        for q in [
            "DELETE FROM click_events WHERE id = $1 AND tenant_id = $2",
            "DELETE FROM click_counters WHERE id = $1 AND tenant_id = $2",
            "DELETE FROM stats_meta WHERE id = $1 AND tenant_id = $2",
        ] {
            sqlx::query(q)
                .bind(id as i64)
                .bind(tenant.0 as i64)
                .execute(&mut *tx)
                .await
                .map_err(StoreError::backend)?;
        }
        tx.commit().await.map_err(StoreError::backend)?;
        Ok(())
    }

    async fn list_broken_link_ids(&self, tenant: TenantId) -> Result<Vec<u64>, StoreError> {
        let rows = with_read!(self, tenant, |c| {
            sqlx::query(
                "SELECT id FROM link_health WHERE tenant_id = $1 AND healthy = false ORDER BY id",
            )
            .bind(tenant.0 as i64)
            .fetch_all(&mut *c)
            .await
        });
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
            out.push(id as u64);
        }
        Ok(out)
    }

    async fn try_acquire_health_lease(
        &self,
        holder: &str,
        ttl_secs: u64,
    ) -> Result<bool, StoreError> {
        // Use the DATABASE clock for both the new expiry and the takeover
        // comparison, so app-node clock skew cannot decide lease ownership.
        let row = sqlx::query(
            "INSERT INTO health_lease (id, holder, expires_at) \
             VALUES (1, $1, EXTRACT(EPOCH FROM now())::bigint + $2) \
             ON CONFLICT (id) DO UPDATE \
               SET holder = $1, expires_at = EXTRACT(EPOCH FROM now())::bigint + $2 \
             WHERE health_lease.expires_at < EXTRACT(EPOCH FROM now())::bigint \
                OR health_lease.holder = $1 \
             RETURNING holder",
        )
        .bind(holder)
        .bind(ttl_secs as i64)
        .fetch_optional(&self.write)
        .await
        .map_err(StoreError::backend)?;
        Ok(row.is_some())
    }

    async fn put_sheets_connection(
        &self,
        tenant: TenantId,
        c: &crate::sheets::SheetsConnection,
    ) -> Result<(), StoreError> {
        // The connection is keyed per tenant (P1b Task 5: `tenant_id` is now
        // the primary key; the legacy `singleton` column is gone).
        // `refresh_token` is sealed at rest (opt-in via `QUARK_ENCRYPTION_KEY`);
        // every other field stays plaintext.
        let sealed = crate::sheets::SheetsConnection {
            refresh_token: secretbox::seal_opt(
                &self.secretbox,
                &c.refresh_token,
                &aad_sheets_refresh_token(tenant),
            ),
            ..c.clone()
        };
        let blob = serde_json::to_value(sealed)?;
        with_write!(self, tenant, |conn| {
            sqlx::query(
                "INSERT INTO sheets_connection (tenant_id, blob) VALUES ($1, $2) \
                 ON CONFLICT (tenant_id) DO UPDATE SET blob = EXCLUDED.blob",
            )
            .bind(tenant.0 as i64)
            .bind(&blob)
            .execute(&mut *conn)
            .await
        });
        Ok(())
    }

    async fn get_sheets_connection(
        &self,
        tenant: TenantId,
    ) -> Result<Option<crate::sheets::SheetsConnection>, StoreError> {
        // Deliberately on the WRITE pool (via `with_write!`): the connector reads
        // this immediately after an OAuth-callback write, so it must be
        // read-your-writes (no replica lag). The tenant-tx still enforces RLS in
        // cloud mode.
        let row = with_write!(self, tenant, |conn| {
            sqlx::query("SELECT blob FROM sheets_connection WHERE tenant_id = $1")
                .bind(tenant.0 as i64)
                .fetch_optional(&mut *conn)
                .await
        });
        match row {
            Some(r) => {
                let blob: serde_json::Value = r.try_get("blob").map_err(StoreError::backend)?;
                let c: crate::sheets::SheetsConnection = serde_json::from_value(blob)?;
                let refresh_token = secretbox::open_opt(
                    &self.secretbox,
                    &c.refresh_token,
                    &aad_sheets_refresh_token(tenant),
                )
                .map_err(StoreError::backend)?;
                Ok(Some(crate::sheets::SheetsConnection { refresh_token, ..c }))
            }
            None => Ok(None),
        }
    }

    async fn delete_sheets_connection(&self, tenant: TenantId) -> Result<(), StoreError> {
        with_write!(self, tenant, |c| {
            sqlx::query("DELETE FROM sheets_connection WHERE tenant_id = $1")
                .bind(tenant.0 as i64)
                .execute(&mut *c)
                .await
        });
        Ok(())
    }

    async fn try_acquire_sheets_lease(
        &self,
        holder: &str,
        ttl_secs: u64,
    ) -> Result<bool, StoreError> {
        // Mirrors `try_acquire_health_lease`: use the DATABASE clock for both the
        // new expiry and the takeover comparison, so app-node clock skew cannot
        // decide lease ownership.
        let row = sqlx::query(
            "INSERT INTO sheets_lease (id, holder, expires_at) \
             VALUES (1, $1, EXTRACT(EPOCH FROM now())::bigint + $2) \
             ON CONFLICT (id) DO UPDATE \
               SET holder = $1, expires_at = EXTRACT(EPOCH FROM now())::bigint + $2 \
             WHERE sheets_lease.expires_at < EXTRACT(EPOCH FROM now())::bigint \
                OR sheets_lease.holder = $1 \
             RETURNING holder",
        )
        .bind(holder)
        .bind(ttl_secs as i64)
        .fetch_optional(&self.write)
        .await
        .map_err(StoreError::backend)?;
        Ok(row.is_some())
    }

    async fn link_health_for(
        &self,
        tenant: TenantId,
        ids: &[u64],
    ) -> Result<Vec<(u64, LinkHealth)>, StoreError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let id_list: Vec<i64> = ids.iter().map(|&i| i as i64).collect();
        let rows = with_read!(self, tenant, |c| {
            sqlx::query(
                "SELECT id, checked_at, status, healthy FROM link_health WHERE tenant_id = $1 AND id = ANY($2)",
            )
            .bind(tenant.0 as i64)
            .bind(&id_list)
            .fetch_all(&mut *c)
            .await
        });
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
            let checked_at: i64 = r.try_get("checked_at").map_err(StoreError::backend)?;
            let status: Option<i32> = r.try_get("status").map_err(StoreError::backend)?;
            let healthy: bool = r.try_get("healthy").map_err(StoreError::backend)?;
            out.push((
                id as u64,
                LinkHealth {
                    checked_at: checked_at as u64,
                    status: status.map(|s| s as u16),
                    healthy,
                },
            ));
        }
        Ok(out)
    }

    async fn next_pixel_id(&self, _tenant: TenantId) -> Result<u64, StoreError> {
        // Global id namespace.
        let row = sqlx::query("SELECT nextval('quark_pixel_id_seq') AS id")
            .fetch_one(&self.write)
            .await
            .map_err(StoreError::backend)?;
        let id: i64 = row.try_get("id").map_err(StoreError::backend)?;
        Ok(id as u64)
    }

    async fn get_pixel(
        &self,
        tenant: TenantId,
        id: u64,
    ) -> Result<Option<PixelConfig>, StoreError> {
        let row = with_read!(self, tenant, |c| {
            sqlx::query(
                "SELECT id, provider, credentials, active, created FROM pixels WHERE tenant_id = $1 AND id = $2",
            )
            .bind(tenant.0 as i64)
            .bind(id as i64)
            .fetch_optional(&mut *c)
            .await
        });
        match row {
            Some(r) => Ok(Some(row_to_pixel(&r)?)),
            None => Ok(None),
        }
    }

    async fn put_pixel(&self, tenant: TenantId, config: &PixelConfig) -> Result<(), StoreError> {
        let credentials = serde_json::to_value(&config.credentials)?;
        with_write!(self, tenant, |c| {
            sqlx::query(
                "INSERT INTO pixels (id, provider, credentials, active, created, tenant_id) VALUES ($1,$2,$3,$4,$5,$6) \
                 ON CONFLICT (id) DO UPDATE SET provider=$2, credentials=$3, active=$4, created=$5, tenant_id=$6",
            )
            .bind(config.id as i64)
            .bind(provider_to_str(config.provider))
            .bind(&credentials)
            .bind(config.active)
            .bind(config.created as i64)
            .bind(tenant.0 as i64)
            .execute(&mut *c)
            .await
        });
        Ok(())
    }

    async fn delete_pixel(&self, tenant: TenantId, id: u64) -> Result<bool, StoreError> {
        let res = with_write!(self, tenant, |c| {
            sqlx::query("DELETE FROM pixels WHERE tenant_id = $1 AND id = $2")
                .bind(tenant.0 as i64)
                .bind(id as i64)
                .execute(&mut *c)
                .await
        });
        Ok(res.rows_affected() > 0)
    }

    async fn list_pixels(&self, tenant: TenantId) -> Result<Vec<PixelConfig>, StoreError> {
        let rows = with_read!(self, tenant, |c| {
            sqlx::query(
                "SELECT id, provider, credentials, active, created FROM pixels WHERE tenant_id = $1 ORDER BY id",
            )
            .bind(tenant.0 as i64)
            .fetch_all(&mut *c)
            .await
        });
        rows.iter().map(row_to_pixel).collect()
    }

    async fn put_alert_rule(
        &self,
        tenant: TenantId,
        link_id: u64,
        rule: &AlertRule,
    ) -> Result<(), StoreError> {
        with_write!(self, tenant, |c| {
            sqlx::query(
                "INSERT INTO alert_rules (tenant_id, link_id, threshold, window_secs) VALUES ($1,$2,$3,$4) \
                 ON CONFLICT (tenant_id, link_id) DO UPDATE SET threshold=$3, window_secs=$4",
            )
            .bind(tenant.0 as i64)
            .bind(link_id as i64)
            .bind(rule.threshold as i64)
            .bind(rule.window_secs as i64)
            .execute(&mut *c)
            .await
        });
        Ok(())
    }

    async fn get_alert_rule(
        &self,
        tenant: TenantId,
        link_id: u64,
    ) -> Result<Option<AlertRule>, StoreError> {
        let row = with_read!(self, tenant, |c| {
            sqlx::query(
                "SELECT threshold, window_secs FROM alert_rules WHERE tenant_id = $1 AND link_id = $2",
            )
            .bind(tenant.0 as i64)
            .bind(link_id as i64)
            .fetch_optional(&mut *c)
            .await
        });
        match row {
            Some(r) => Ok(Some(row_to_alert_rule(&r))),
            None => Ok(None),
        }
    }

    async fn delete_alert_rule(&self, tenant: TenantId, link_id: u64) -> Result<(), StoreError> {
        with_write!(self, tenant, |c| {
            sqlx::query("DELETE FROM alert_rules WHERE tenant_id = $1 AND link_id = $2")
                .bind(tenant.0 as i64)
                .bind(link_id as i64)
                .execute(&mut *c)
                .await
        });
        Ok(())
    }

    async fn list_alert_rules(
        &self,
        tenant: TenantId,
    ) -> Result<Vec<(u64, AlertRule)>, StoreError> {
        let rows = with_read!(self, tenant, |c| {
            sqlx::query(
                "SELECT link_id, threshold, window_secs FROM alert_rules WHERE tenant_id = $1 ORDER BY link_id",
            )
            .bind(tenant.0 as i64)
            .fetch_all(&mut *c)
            .await
        });
        Ok(rows
            .iter()
            .map(|r| {
                let link_id: i64 = r.get("link_id");
                (link_id as u64, row_to_alert_rule(r))
            })
            .collect())
    }

    async fn get_wellknown(
        &self,
        tenant: TenantId,
        name: &str,
    ) -> Result<Option<String>, StoreError> {
        let row = with_read!(self, tenant, |c| {
            sqlx::query("SELECT body FROM wellknown_documents WHERE tenant_id = $1 AND name = $2")
                .bind(tenant.0 as i64)
                .bind(name)
                .fetch_optional(&mut *c)
                .await
        });
        match row {
            Some(r) => {
                let body: String = r.try_get("body").map_err(StoreError::backend)?;
                Ok(Some(body))
            }
            None => Ok(None),
        }
    }

    async fn put_wellknown(
        &self,
        tenant: TenantId,
        name: &str,
        body: &str,
    ) -> Result<(), StoreError> {
        with_write!(self, tenant, |c| {
            sqlx::query(
                "INSERT INTO wellknown_documents (name, body, tenant_id) VALUES ($1,$2,$3) \
                 ON CONFLICT (tenant_id, name) DO UPDATE SET body = EXCLUDED.body",
            )
            .bind(name)
            .bind(body)
            .bind(tenant.0 as i64)
            .execute(&mut *c)
            .await
        });
        Ok(())
    }

    async fn delete_wellknown(&self, tenant: TenantId, name: &str) -> Result<(), StoreError> {
        with_write!(self, tenant, |c| {
            sqlx::query("DELETE FROM wellknown_documents WHERE tenant_id = $1 AND name = $2")
                .bind(tenant.0 as i64)
                .bind(name)
                .execute(&mut *c)
                .await
        });
        Ok(())
    }

    // --- Identity / tenancy ---
    async fn put_tenant(&self, t: &Tenant) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO tenants (id, name, slug, created) VALUES ($1,$2,$3,$4) \
             ON CONFLICT (id) DO UPDATE SET name=$2, slug=$3, created=$4",
        )
        .bind(t.id.0 as i64)
        .bind(&t.name)
        .bind(&t.slug)
        .bind(t.created as i64)
        .execute(&self.write)
        .await
        // `id` is a fresh sequence value (never conflicts), so the only
        // constraint this insert can actually hit is the `slug` UNIQUE — a
        // Postgres unique-violation (SQLSTATE 23505) surfaces distinctly so
        // the caller can map it to 409 instead of 503.
        .map_err(map_unique_violation)?;
        Ok(())
    }

    async fn get_tenant(&self, id: TenantId) -> Result<Option<Tenant>, StoreError> {
        let row = sqlx::query("SELECT id, name, slug, created FROM tenants WHERE id = $1")
            .bind(id.0 as i64)
            .fetch_optional(&self.read)
            .await
            .map_err(StoreError::backend)?;
        match row {
            Some(r) => {
                let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
                let created: i64 = r.try_get("created").map_err(StoreError::backend)?;
                Ok(Some(Tenant {
                    id: TenantId(id as u64),
                    name: r.try_get("name").map_err(StoreError::backend)?,
                    slug: r.try_get("slug").map_err(StoreError::backend)?,
                    created: created as u64,
                }))
            }
            None => Ok(None),
        }
    }

    async fn get_tenant_by_slug(&self, slug: &str) -> Result<Option<Tenant>, StoreError> {
        // Bare pool, deliberately: `/admin/login?org=<slug>` resolves the
        // tenant before any session/RLS context exists, mirroring
        // `get_domain_by_host`.
        let row = sqlx::query("SELECT id, name, slug, created FROM tenants WHERE slug = $1")
            .bind(slug)
            .fetch_optional(&self.read)
            .await
            .map_err(StoreError::backend)?;
        match row {
            Some(r) => {
                let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
                let created: i64 = r.try_get("created").map_err(StoreError::backend)?;
                Ok(Some(Tenant {
                    id: TenantId(id as u64),
                    name: r.try_get("name").map_err(StoreError::backend)?,
                    slug: r.try_get("slug").map_err(StoreError::backend)?,
                    created: created as u64,
                }))
            }
            None => Ok(None),
        }
    }

    async fn reencrypt_legacy_secrets(&self) -> Result<usize, StoreError> {
        // No key configured: nothing to do, and no round-trip worth paying for.
        let Some(sb) = self.secretbox.as_ref() else {
            return Ok(0);
        };
        let primary_keyid = sb.primary_keyid();
        let mut count = 0usize;

        // oidc_configs: `NOT_FORCED` (see the const above), so a bare scan on
        // the read pool sees every tenant's row regardless of RLS context —
        // same reasoning `get_oidc_config_bare`/`get_tenant_by_slug` rely on.
        let rows =
            sqlx::query("SELECT tenant_id, blob->>'client_secret' AS secret FROM oidc_configs")
                .fetch_all(&self.read)
                .await
                .map_err(StoreError::backend)?;
        for r in rows {
            let tenant_id: i64 = r.try_get("tenant_id").map_err(StoreError::backend)?;
            let secret: Option<String> = r.try_get("secret").map_err(StoreError::backend)?;
            if !needs_reseal(&secret, primary_keyid) {
                continue;
            }
            let tenant = TenantId(tenant_id as u64);
            // get→put round-trip: `get_oidc_config_bare` opens (plaintext passes
            // through, v1/old-v2 decrypt via the keyring), `put_oidc_config`
            // re-seals under the primary key with the correct per-row AAD.
            // No hand-rolled crypto here, just the existing seal/open path.
            if let Some(config) = self.get_oidc_config_bare(tenant).await? {
                self.put_oidc_config(&config).await?;
                count += 1;
            }
        }

        // sheets_connection IS force-RLS'd in multi-tenant mode (it is not in
        // `NOT_FORCED`), so a bare scan would silently miss every tenant's row
        // there. Enumerate tenants instead and check each one inside its own
        // tenant-scoped transaction, mirroring `get_sheets_connection`'s own
        // read path (`with_write!`, for read-your-writes).
        let tenants = self.list_tenants().await?;
        for t in &tenants {
            let row = with_write!(self, t.id, |c| {
                sqlx::query(
                    "SELECT blob->>'refresh_token' AS secret FROM sheets_connection WHERE tenant_id = $1",
                )
                .bind(t.id.0 as i64)
                .fetch_optional(&mut *c)
                .await
            });
            let Some(row) = row else {
                continue;
            };
            let secret: Option<String> = row.try_get("secret").map_err(StoreError::backend)?;
            if !needs_reseal(&secret, primary_keyid) {
                continue;
            }
            if let Some(conn) = self.get_sheets_connection(t.id).await? {
                self.put_sheets_connection(t.id, &conn).await?;
                count += 1;
            }
        }

        Ok(count)
    }

    async fn list_tenants(&self) -> Result<Vec<Tenant>, StoreError> {
        let rows = sqlx::query("SELECT id, name, slug, created FROM tenants ORDER BY id")
            .fetch_all(&self.read)
            .await
            .map_err(StoreError::backend)?;
        rows.iter()
            .map(|r| {
                let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
                let created: i64 = r.try_get("created").map_err(StoreError::backend)?;
                Ok(Tenant {
                    id: TenantId(id as u64),
                    name: r.try_get("name").map_err(StoreError::backend)?,
                    slug: r.try_get("slug").map_err(StoreError::backend)?,
                    created: created as u64,
                })
            })
            .collect()
    }

    async fn next_user_id(&self) -> Result<u64, StoreError> {
        let row = sqlx::query("SELECT nextval('quark_user_id_seq') AS id")
            .fetch_one(&self.write)
            .await
            .map_err(StoreError::backend)?;
        let id: i64 = row.try_get("id").map_err(StoreError::backend)?;
        Ok(id as u64)
    }

    async fn next_tenant_id(&self) -> Result<u64, StoreError> {
        let row = sqlx::query("SELECT nextval('quark_tenant_id_seq') AS id")
            .fetch_one(&self.write)
            .await
            .map_err(StoreError::backend)?;
        let id: i64 = row.try_get("id").map_err(StoreError::backend)?;
        Ok(id as u64)
    }

    async fn put_user(&self, u: &User) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO users (id, subject, email, display, created) VALUES ($1,$2,$3,$4,$5) \
             ON CONFLICT (id) DO UPDATE SET subject=$2, email=$3, display=$4, created=$5",
        )
        .bind(u.id as i64)
        .bind(&u.subject)
        .bind(&u.email)
        .bind(&u.display)
        .bind(u.created as i64)
        .execute(&self.write)
        .await
        .map_err(StoreError::backend)?;
        Ok(())
    }

    async fn get_user_by_subject(&self, subject: &str) -> Result<Option<User>, StoreError> {
        let row = sqlx::query(
            "SELECT id, subject, email, display, created FROM users WHERE subject = $1",
        )
        .bind(subject)
        .fetch_optional(&self.read)
        .await
        .map_err(StoreError::backend)?;
        match row {
            Some(r) => {
                let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
                let created: i64 = r.try_get("created").map_err(StoreError::backend)?;
                Ok(Some(User {
                    id: id as u64,
                    subject: r.try_get("subject").map_err(StoreError::backend)?,
                    email: r.try_get("email").map_err(StoreError::backend)?,
                    display: r.try_get("display").map_err(StoreError::backend)?,
                    created: created as u64,
                }))
            }
            None => Ok(None),
        }
    }

    async fn get_user_by_id(&self, id: u64) -> Result<Option<User>, StoreError> {
        let row =
            sqlx::query("SELECT id, subject, email, display, created FROM users WHERE id = $1")
                .bind(id as i64)
                .fetch_optional(&self.read)
                .await
                .map_err(StoreError::backend)?;
        match row {
            Some(r) => {
                let id: i64 = r.try_get("id").map_err(StoreError::backend)?;
                let created: i64 = r.try_get("created").map_err(StoreError::backend)?;
                Ok(Some(User {
                    id: id as u64,
                    subject: r.try_get("subject").map_err(StoreError::backend)?,
                    email: r.try_get("email").map_err(StoreError::backend)?,
                    display: r.try_get("display").map_err(StoreError::backend)?,
                    created: created as u64,
                }))
            }
            None => Ok(None),
        }
    }

    async fn put_membership(&self, m: &Membership) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO memberships (user_id, tenant_id, role, created) VALUES ($1,$2,$3,$4) \
             ON CONFLICT (user_id, tenant_id) DO UPDATE SET role=$3, created=$4",
        )
        .bind(m.user_id as i64)
        .bind(m.tenant_id.0 as i64)
        .bind(role_to_str(m.role))
        .bind(m.created as i64)
        .execute(&self.write)
        .await
        .map_err(StoreError::backend)?;
        Ok(())
    }

    async fn get_membership(
        &self,
        user_id: u64,
        tenant: TenantId,
    ) -> Result<Option<Membership>, StoreError> {
        let row = sqlx::query(
            "SELECT user_id, tenant_id, role, created FROM memberships WHERE user_id = $1 AND tenant_id = $2",
        )
        .bind(user_id as i64)
        .bind(tenant.0 as i64)
        .fetch_optional(&self.read)
        .await
        .map_err(StoreError::backend)?;
        match row {
            Some(r) => Ok(Some(row_to_membership(&r)?)),
            None => Ok(None),
        }
    }

    async fn list_memberships_for_user(&self, user_id: u64) -> Result<Vec<Membership>, StoreError> {
        let rows = sqlx::query(
            "SELECT user_id, tenant_id, role, created FROM memberships WHERE user_id = $1 ORDER BY tenant_id",
        )
        .bind(user_id as i64)
        .fetch_all(&self.read)
        .await
        .map_err(StoreError::backend)?;
        rows.iter().map(row_to_membership).collect()
    }

    async fn get_owner_user_id(&self, tenant: TenantId) -> Result<Option<u64>, StoreError> {
        // Bare read (like `get_membership`): the backfill runs at boot before
        // any request/tenant RLS context. `memberships` is a join table (not in
        // TENANT_OWNED_TABLES), and the `WHERE tenant_id` predicate scopes it.
        let row = sqlx::query(
            "SELECT user_id FROM memberships WHERE tenant_id = $1 AND role = $2 ORDER BY user_id LIMIT 1",
        )
        .bind(tenant.0 as i64)
        .bind(role_to_str(Role::Owner))
        .fetch_optional(&self.read)
        .await
        .map_err(StoreError::backend)?;
        match row {
            Some(r) => {
                let uid: i64 = r.try_get("user_id").map_err(StoreError::backend)?;
                Ok(Some(uid as u64))
            }
            None => Ok(None),
        }
    }

    // --- Custom domains (P3), cloud-only ---
    async fn next_domain_id(&self) -> Result<u64, StoreError> {
        let row = sqlx::query("SELECT nextval('quark_domain_id_seq') AS id")
            .fetch_one(&self.write)
            .await
            .map_err(StoreError::backend)?;
        let id: i64 = row.try_get("id").map_err(StoreError::backend)?;
        Ok(id as u64)
    }

    async fn get_domain_by_host(&self, host: &str) -> Result<Option<Domain>, StoreError> {
        // Deliberately bare: no tenant transaction, no `app.tenant_id`. The
        // redirect path only has a `Host` header before it knows the tenant,
        // so this runs on the read pool directly (`domains` is in
        // `NOT_FORCED` for exactly this reason).
        let row = sqlx::query(
            "SELECT id, tenant_id, host, token, status, created, verified_at FROM domains WHERE host = $1",
        )
        .bind(host)
        .fetch_optional(&self.read)
        .await
        .map_err(StoreError::backend)?;
        match row {
            Some(r) => Ok(Some(row_to_domain(&r)?)),
            None => Ok(None),
        }
    }

    async fn get_domain(&self, tenant: TenantId, id: u64) -> Result<Option<Domain>, StoreError> {
        let row = with_read!(self, tenant, |c| {
            sqlx::query(
                "SELECT id, tenant_id, host, token, status, created, verified_at FROM domains WHERE tenant_id = $1 AND id = $2",
            )
            .bind(tenant.0 as i64)
            .bind(id as i64)
            .fetch_optional(&mut *c)
            .await
        });
        match row {
            Some(r) => Ok(Some(row_to_domain(&r)?)),
            None => Ok(None),
        }
    }

    async fn list_domains(&self, tenant: TenantId) -> Result<Vec<Domain>, StoreError> {
        let rows = with_read!(self, tenant, |c| {
            sqlx::query(
                "SELECT id, tenant_id, host, token, status, created, verified_at FROM domains WHERE tenant_id = $1 ORDER BY id",
            )
            .bind(tenant.0 as i64)
            .fetch_all(&mut *c)
            .await
        });
        rows.iter().map(row_to_domain).collect()
    }

    async fn put_domain(&self, domain: &Domain) -> Result<(), StoreError> {
        let status = match domain.status {
            DomainStatus::Pending => "pending",
            DomainStatus::Verified => "verified",
        };
        const SQL: &str =
            "INSERT INTO domains (id, tenant_id, host, token, status, created, verified_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7) \
             ON CONFLICT (id) DO UPDATE SET \
               tenant_id = EXCLUDED.tenant_id, host = EXCLUDED.host, token = EXCLUDED.token, \
               status = EXCLUDED.status, created = EXCLUDED.created, verified_at = EXCLUDED.verified_at";
        // `id` is a fresh sequence value (never conflicts), so the only
        // constraint this insert can actually hit is the `host` UNIQUE — a
        // Postgres unique-violation (SQLSTATE 23505) surfaces distinctly so
        // the caller can map it to 409 instead of 503, same as `put_tenant`.
        // Written out by hand (not `with_write!`) because that macro folds
        // every error into `StoreError::Backend` before it reaches here,
        // losing the SQLSTATE this needs to inspect.
        if self.multi_tenant {
            let mut tx = self.begin_tenant_tx(domain.tenant_id).await?;
            sqlx::query(SQL)
                .bind(domain.id as i64)
                .bind(domain.tenant_id.0 as i64)
                .bind(&domain.host)
                .bind(&domain.token)
                .bind(status)
                .bind(domain.created as i64)
                .bind(domain.verified_at.map(|v| v as i64))
                .execute(&mut *tx)
                .await
                .map_err(map_unique_violation)?;
            tx.commit().await.map_err(StoreError::backend)?;
        } else {
            let mut conn = self.write.acquire().await.map_err(StoreError::backend)?;
            sqlx::query(SQL)
                .bind(domain.id as i64)
                .bind(domain.tenant_id.0 as i64)
                .bind(&domain.host)
                .bind(&domain.token)
                .bind(status)
                .bind(domain.created as i64)
                .bind(domain.verified_at.map(|v| v as i64))
                .execute(&mut *conn)
                .await
                .map_err(map_unique_violation)?;
        }
        Ok(())
    }

    async fn set_domain_status(
        &self,
        tenant: TenantId,
        id: u64,
        status: DomainStatus,
        verified_at: Option<u64>,
    ) -> Result<(), StoreError> {
        let status = match status {
            DomainStatus::Pending => "pending",
            DomainStatus::Verified => "verified",
        };
        with_write!(self, tenant, |c| {
            sqlx::query(
                "UPDATE domains SET status = $1, verified_at = $2 WHERE tenant_id = $3 AND id = $4",
            )
            .bind(status)
            .bind(verified_at.map(|v| v as i64))
            .bind(tenant.0 as i64)
            .bind(id as i64)
            .execute(&mut *c)
            .await
        });
        Ok(())
    }

    async fn delete_domain(&self, tenant: TenantId, id: u64) -> Result<(), StoreError> {
        with_write!(self, tenant, |c| {
            sqlx::query("DELETE FROM domains WHERE tenant_id = $1 AND id = $2")
                .bind(tenant.0 as i64)
                .bind(id as i64)
                .execute(&mut *c)
                .await
        });
        Ok(())
    }

    async fn set_primary_domain(
        &self,
        tenant: TenantId,
        domain_id: Option<u64>,
    ) -> Result<(), StoreError> {
        // `tenants` is a global table (not RLS-scoped), so mirror `put_tenant`'s
        // bare-pool access rather than `with_write!`. `None` clears the primary.
        sqlx::query("UPDATE tenants SET primary_domain_id = $2 WHERE id = $1")
            .bind(tenant.0 as i64)
            .bind(domain_id.map(|d| d as i64))
            .execute(&self.write)
            .await
            .map_err(StoreError::backend)?;
        Ok(())
    }

    async fn get_primary_domain_id(&self, tenant: TenantId) -> Result<Option<u64>, StoreError> {
        let row = sqlx::query("SELECT primary_domain_id FROM tenants WHERE id = $1")
            .bind(tenant.0 as i64)
            .fetch_optional(&self.write)
            .await
            .map_err(StoreError::backend)?;
        match row {
            Some(r) => {
                let id: Option<i64> = r.try_get("primary_domain_id").map_err(StoreError::backend)?;
                Ok(id.map(|v| v as u64))
            }
            None => Ok(None),
        }
    }

    // --- SSO email-domain discovery (LUC-57), cloud-only ---
    async fn next_sso_domain_id(&self) -> Result<u64, StoreError> {
        let row = sqlx::query("SELECT nextval('quark_sso_domain_id_seq') AS id")
            .fetch_one(&self.write)
            .await
            .map_err(StoreError::backend)?;
        let id: i64 = row.try_get("id").map_err(StoreError::backend)?;
        Ok(id as u64)
    }

    async fn get_sso_domain_bare(
        &self,
        domain: &str,
    ) -> Result<Option<SsoEmailDomain>, StoreError> {
        // Deliberately bare: discovery only has the email's domain before it
        // knows the tenant, so this runs on the read pool directly
        // (`sso_email_domains` is in `NOT_FORCED` for exactly this reason).
        let row = sqlx::query(
            "SELECT id, tenant_id, domain, token, status, created, verified_at FROM sso_email_domains WHERE domain = $1",
        )
        .bind(domain)
        .fetch_optional(&self.read)
        .await
        .map_err(StoreError::backend)?;
        match row {
            Some(r) => Ok(Some(row_to_sso_domain(&r)?)),
            None => Ok(None),
        }
    }

    async fn get_sso_domain(
        &self,
        tenant: TenantId,
        id: u64,
    ) -> Result<Option<SsoEmailDomain>, StoreError> {
        let row = with_read!(self, tenant, |c| {
            sqlx::query(
                "SELECT id, tenant_id, domain, token, status, created, verified_at FROM sso_email_domains WHERE tenant_id = $1 AND id = $2",
            )
            .bind(tenant.0 as i64)
            .bind(id as i64)
            .fetch_optional(&mut *c)
            .await
        });
        match row {
            Some(r) => Ok(Some(row_to_sso_domain(&r)?)),
            None => Ok(None),
        }
    }

    async fn list_sso_domains(&self, tenant: TenantId) -> Result<Vec<SsoEmailDomain>, StoreError> {
        let rows = with_read!(self, tenant, |c| {
            sqlx::query(
                "SELECT id, tenant_id, domain, token, status, created, verified_at FROM sso_email_domains WHERE tenant_id = $1 ORDER BY id",
            )
            .bind(tenant.0 as i64)
            .fetch_all(&mut *c)
            .await
        });
        rows.iter().map(row_to_sso_domain).collect()
    }

    async fn put_sso_domain(&self, domain: &SsoEmailDomain) -> Result<(), StoreError> {
        let status = match domain.status {
            DomainStatus::Pending => "pending",
            DomainStatus::Verified => "verified",
        };
        const SQL: &str =
            "INSERT INTO sso_email_domains (id, tenant_id, domain, token, status, created, verified_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7) \
             ON CONFLICT (id) DO UPDATE SET \
               tenant_id = EXCLUDED.tenant_id, domain = EXCLUDED.domain, token = EXCLUDED.token, \
               status = EXCLUDED.status, created = EXCLUDED.created, verified_at = EXCLUDED.verified_at";
        // `id` is a fresh sequence value (never conflicts), so the only
        // constraint this insert can hit is the `domain` UNIQUE — a Postgres
        // unique-violation (SQLSTATE 23505) surfaces distinctly so the caller
        // can map it to 409, same as `put_domain`'s `host`. Written by hand
        // (not `with_write!`) because that macro folds every error into
        // `StoreError::Backend`, losing the SQLSTATE this needs.
        if self.multi_tenant {
            let mut tx = self.begin_tenant_tx(domain.tenant_id).await?;
            sqlx::query(SQL)
                .bind(domain.id as i64)
                .bind(domain.tenant_id.0 as i64)
                .bind(&domain.domain)
                .bind(&domain.token)
                .bind(status)
                .bind(domain.created as i64)
                .bind(domain.verified_at.map(|v| v as i64))
                .execute(&mut *tx)
                .await
                .map_err(map_unique_violation)?;
            tx.commit().await.map_err(StoreError::backend)?;
        } else {
            let mut conn = self.write.acquire().await.map_err(StoreError::backend)?;
            sqlx::query(SQL)
                .bind(domain.id as i64)
                .bind(domain.tenant_id.0 as i64)
                .bind(&domain.domain)
                .bind(&domain.token)
                .bind(status)
                .bind(domain.created as i64)
                .bind(domain.verified_at.map(|v| v as i64))
                .execute(&mut *conn)
                .await
                .map_err(map_unique_violation)?;
        }
        Ok(())
    }

    async fn set_sso_domain_status(
        &self,
        tenant: TenantId,
        id: u64,
        status: DomainStatus,
        verified_at: Option<u64>,
    ) -> Result<(), StoreError> {
        let status = match status {
            DomainStatus::Pending => "pending",
            DomainStatus::Verified => "verified",
        };
        with_write!(self, tenant, |c| {
            sqlx::query(
                "UPDATE sso_email_domains SET status = $1, verified_at = $2 WHERE tenant_id = $3 AND id = $4",
            )
            .bind(status)
            .bind(verified_at.map(|v| v as i64))
            .bind(tenant.0 as i64)
            .bind(id as i64)
            .execute(&mut *c)
            .await
        });
        Ok(())
    }

    async fn delete_sso_domain(&self, tenant: TenantId, id: u64) -> Result<(), StoreError> {
        with_write!(self, tenant, |c| {
            sqlx::query("DELETE FROM sso_email_domains WHERE tenant_id = $1 AND id = $2")
                .bind(tenant.0 as i64)
                .bind(id as i64)
                .execute(&mut *c)
                .await
        });
        Ok(())
    }

    // --- Team invites (P2c), cloud-only ---
    async fn next_invite_id(&self) -> Result<u64, StoreError> {
        let row = sqlx::query("SELECT nextval('quark_invite_id_seq') AS id")
            .fetch_one(&self.write)
            .await
            .map_err(StoreError::backend)?;
        let id: i64 = row.try_get("id").map_err(StoreError::backend)?;
        Ok(id as u64)
    }

    async fn create_invite(&self, inv: &Invite) -> Result<(), StoreError> {
        // Deliberately bare: no tenant transaction. The accept flow only has
        // the raw token before it knows the tenant, so `create_invite` writes
        // on the same bare pool `get_invite_by_hash` reads from.
        sqlx::query(
            "INSERT INTO invites (id, tenant_id, email, role, token_hash, invited_by, created, expires, accepted_at, accepted_by) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
        )
        .bind(inv.id as i64)
        .bind(inv.tenant_id.0 as i64)
        .bind(&inv.email)
        .bind(role_to_str(inv.role))
        .bind(&inv.token_hash)
        .bind(inv.invited_by as i64)
        .bind(inv.created as i64)
        .bind(inv.expires as i64)
        .bind(inv.accepted_at.map(|v| v as i64))
        .bind(inv.accepted_by.map(|v| v as i64))
        .execute(&self.write)
        .await
        .map_err(StoreError::backend)?;
        Ok(())
    }

    /// Hot path of accept: indexed lookup by `token_hash`
    /// (`invites_token_hash_idx`), filtered to pending and unexpired.
    async fn get_invite_by_hash(
        &self,
        token_hash: &str,
        now: u64,
    ) -> Result<Option<Invite>, StoreError> {
        let row = sqlx::query(
            "SELECT id, tenant_id, email, role, token_hash, invited_by, created, expires, accepted_at, accepted_by \
             FROM invites WHERE token_hash = $1 AND accepted_at IS NULL AND expires >= $2",
        )
        .bind(token_hash)
        .bind(now as i64)
        .fetch_optional(&self.read)
        .await
        .map_err(StoreError::backend)?;
        match row {
            Some(r) => Ok(Some(row_to_invite(&r)?)),
            None => Ok(None),
        }
    }

    async fn mark_invite_accepted(
        &self,
        id: u64,
        accepted_by: u64,
        now: u64,
    ) -> Result<bool, StoreError> {
        let result = sqlx::query(
            "UPDATE invites SET accepted_at = $1, accepted_by = $2 \
             WHERE id = $3 AND accepted_at IS NULL",
        )
        .bind(now as i64)
        .bind(accepted_by as i64)
        .bind(id as i64)
        .execute(&self.write)
        .await
        .map_err(StoreError::backend)?;
        Ok(result.rows_affected() == 1)
    }

    async fn accept_invite_tx(
        &self,
        invite_id: u64,
        membership: &Membership,
        now: u64,
    ) -> Result<bool, StoreError> {
        // Same claim query as `mark_invite_accepted`, and the same INSERT as
        // `put_membership`, but both inside one `begin_tenant_tx` transaction
        // (mirrors `put_link_tx`/the other `_tx` methods): if the membership
        // INSERT below errors, the `?` returns before `commit()` and the
        // transaction (including the claim UPDATE) is rolled back on drop, so
        // a failed grant never leaves the invite consumed.
        let mut tx = self.begin_tenant_tx(membership.tenant_id).await?;
        let claim = sqlx::query(
            "UPDATE invites SET accepted_at = $1, accepted_by = $2 \
             WHERE id = $3 AND accepted_at IS NULL",
        )
        .bind(now as i64)
        .bind(membership.user_id as i64)
        .bind(invite_id as i64)
        .execute(&mut *tx)
        .await
        .map_err(StoreError::backend)?;
        if claim.rows_affected() == 0 {
            // Lost the race / already consumed: nothing to grant. Commit the
            // (no-op) transaction rather than rolling it back, matching
            // `mark_invite_accepted`'s Ok(false) semantics.
            tx.commit().await.map_err(StoreError::backend)?;
            return Ok(false);
        }
        sqlx::query(
            "INSERT INTO memberships (user_id, tenant_id, role, created) VALUES ($1,$2,$3,$4) \
             ON CONFLICT (user_id, tenant_id) DO UPDATE SET role=$3, created=$4",
        )
        .bind(membership.user_id as i64)
        .bind(membership.tenant_id.0 as i64)
        .bind(role_to_str(membership.role))
        .bind(membership.created as i64)
        .execute(&mut *tx)
        .await
        .map_err(StoreError::backend)?;
        tx.commit().await.map_err(StoreError::backend)?;
        Ok(true)
    }

    async fn list_invites(&self, tenant: TenantId) -> Result<Vec<Invite>, StoreError> {
        let rows = with_read!(self, tenant, |c| {
            sqlx::query(
                "SELECT id, tenant_id, email, role, token_hash, invited_by, created, expires, accepted_at, accepted_by \
                 FROM invites WHERE tenant_id = $1 AND accepted_at IS NULL ORDER BY id",
            )
            .bind(tenant.0 as i64)
            .fetch_all(&mut *c)
            .await
        });
        rows.iter().map(row_to_invite).collect()
    }

    async fn delete_invite(&self, tenant: TenantId, id: u64) -> Result<(), StoreError> {
        with_write!(self, tenant, |c| {
            sqlx::query("DELETE FROM invites WHERE tenant_id = $1 AND id = $2")
                .bind(tenant.0 as i64)
                .bind(id as i64)
                .execute(&mut *c)
                .await
        });
        Ok(())
    }

    // --- Per-tenant OIDC config (multi-tenancy P2d), cloud-only ---
    async fn next_oidc_config_id(&self) -> Result<u64, StoreError> {
        let row = sqlx::query("SELECT nextval('quark_oidc_config_id_seq') AS id")
            .fetch_one(&self.write)
            .await
            .map_err(StoreError::backend)?;
        let id: i64 = row.try_get("id").map_err(StoreError::backend)?;
        Ok(id as u64)
    }

    async fn put_oidc_config(&self, cfg: &TenantOidcConfig) -> Result<(), StoreError> {
        // `id` is a fresh sequence value used only on first insert for this
        // tenant: the upsert conflicts on the UNIQUE `tenant_id`, so a
        // pre-existing row keeps its original `id`/`created` and only
        // `issuer`/`blob` are replaced.
        let id = self.next_oidc_config_id().await?;
        let blob = oidc_config_blob(cfg, &self.secretbox);
        let created = crate::now() as i64;
        with_write!(self, cfg.tenant_id, |c| {
            sqlx::query(
                "INSERT INTO oidc_configs (id, tenant_id, issuer, blob, created) \
                 VALUES ($1,$2,$3,$4,$5) \
                 ON CONFLICT (tenant_id) DO UPDATE SET issuer = EXCLUDED.issuer, blob = EXCLUDED.blob",
            )
            .bind(id as i64)
            .bind(cfg.tenant_id.0 as i64)
            .bind(&cfg.issuer)
            .bind(&blob)
            .bind(created)
            .execute(&mut *c)
            .await
        });
        Ok(())
    }

    async fn get_oidc_config(
        &self,
        tenant: TenantId,
    ) -> Result<Option<TenantOidcConfig>, StoreError> {
        let row = with_read!(self, tenant, |c| {
            sqlx::query("SELECT tenant_id, issuer, blob FROM oidc_configs WHERE tenant_id = $1")
                .bind(tenant.0 as i64)
                .fetch_optional(&mut *c)
                .await
        });
        match row {
            Some(r) => Ok(Some(row_to_oidc_config(&r, &self.secretbox)?)),
            None => Ok(None),
        }
    }

    async fn get_oidc_config_bare(
        &self,
        tenant: TenantId,
    ) -> Result<Option<TenantOidcConfig>, StoreError> {
        // Deliberately bare: the login/callback path resolves the tenant from
        // the URL slug (`get_tenant_by_slug`) before there is any
        // `app.tenant_id` RLS context to scope through (`oidc_configs` is in
        // `NOT_FORCED` for exactly this reason).
        let row =
            sqlx::query("SELECT tenant_id, issuer, blob FROM oidc_configs WHERE tenant_id = $1")
                .bind(tenant.0 as i64)
                .fetch_optional(&self.read)
                .await
                .map_err(StoreError::backend)?;
        match row {
            Some(r) => Ok(Some(row_to_oidc_config(&r, &self.secretbox)?)),
            None => Ok(None),
        }
    }

    async fn delete_oidc_config(&self, tenant: TenantId) -> Result<(), StoreError> {
        with_write!(self, tenant, |c| {
            sqlx::query("DELETE FROM oidc_configs WHERE tenant_id = $1")
                .bind(tenant.0 as i64)
                .execute(&mut *c)
                .await
        });
        Ok(())
    }

    async fn update_oidc_config_issuer(
        &self,
        tenant: TenantId,
        issuer: &str,
    ) -> Result<(), StoreError> {
        // Only the `issuer` column is rewritten; the `blob` (which carries the
        // encrypted-at-rest `client_secret`) is never touched, so the secret
        // cannot be lost in this reconcile (LUC-81). A tenant with no config
        // matches zero rows and is a no-op, not an error.
        with_write!(self, tenant, |c| {
            sqlx::query("UPDATE oidc_configs SET issuer = $2 WHERE tenant_id = $1")
                .bind(tenant.0 as i64)
                .bind(issuer)
                .execute(&mut *c)
                .await
        });
        Ok(())
    }

    async fn update_oidc_config_member_value(
        &self,
        tenant: TenantId,
        member_value: &str,
    ) -> Result<(), StoreError> {
        // Rewrites only the blob's `member_value` key via `jsonb_set`; every
        // other key (including the encrypted `client_secret`) is preserved, so
        // this reconcile cannot lose the secret. A tenant with no config
        // matches zero rows and is a no-op, not an error.
        with_write!(self, tenant, |c| {
            sqlx::query(
                "UPDATE oidc_configs SET blob = jsonb_set(blob, '{member_value}', to_jsonb($2::text)) WHERE tenant_id = $1",
            )
            .bind(tenant.0 as i64)
            .bind(member_value)
            .execute(&mut *c)
            .await
        });
        Ok(())
    }

    async fn enqueue_deliveries(&self, rows: &[OutboxRow]) -> Result<(), StoreError> {
        if rows.is_empty() {
            return Ok(());
        }
        let mut tx = self.write.begin().await.map_err(StoreError::backend)?;
        for row in rows {
            sqlx::query(
                "INSERT INTO webhook_deliveries (delivery_key, subscription_id, event_type, payload, created, next_attempt_at, tenant_id) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7) ON CONFLICT (delivery_key) DO NOTHING",
            )
            .bind(&row.delivery_key)
            .bind(row.subscription_id as i64)
            .bind(&row.event_type)
            .bind(&row.payload)
            .bind(row.created as i64)
            .bind(row.next_attempt_at as i64)
            .bind(row.tenant_id.0 as i64)
            .execute(&mut *tx)
            .await
            .map_err(StoreError::backend)?;
        }
        tx.commit().await.map_err(StoreError::backend)?;
        Ok(())
    }

    async fn claim_due_deliveries(
        &self,
        now: u64,
        limit: i64,
    ) -> Result<Vec<OutboxDelivery>, StoreError> {
        let lease_until = now.saturating_add(CLAIM_LEASE_SECS);
        let rows = sqlx::query(
            "UPDATE webhook_deliveries SET next_attempt_at = $1 \
             WHERE id IN ( \
                 SELECT id FROM webhook_deliveries \
                 WHERE dead = false AND delivered_at IS NULL AND next_attempt_at <= $2 \
                 ORDER BY next_attempt_at \
                 FOR UPDATE SKIP LOCKED \
                 LIMIT $3 \
             ) \
             RETURNING id, delivery_key, subscription_id, event_type, payload, attempts, tenant_id",
        )
        .bind(lease_until as i64)
        .bind(now as i64)
        .bind(limit)
        .fetch_all(&self.write)
        .await
        .map_err(StoreError::backend)?;
        rows.iter().map(row_to_delivery).collect()
    }

    async fn mark_delivered(&self, id: i64) -> Result<(), StoreError> {
        sqlx::query("UPDATE webhook_deliveries SET delivered_at = $1 WHERE id = $2")
            .bind(crate::now() as i64)
            .bind(id)
            .execute(&self.write)
            .await
            .map_err(StoreError::backend)?;
        Ok(())
    }

    async fn mark_retry(
        &self,
        id: i64,
        next_attempt_at: u64,
        attempts: u32,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE webhook_deliveries SET next_attempt_at = $1, attempts = $2 WHERE id = $3",
        )
        .bind(next_attempt_at as i64)
        .bind(attempts as i32)
        .bind(id)
        .execute(&self.write)
        .await
        .map_err(StoreError::backend)?;
        Ok(())
    }

    async fn mark_dead(&self, id: i64, attempts: u32) -> Result<(), StoreError> {
        sqlx::query("UPDATE webhook_deliveries SET dead = true, attempts = $1 WHERE id = $2")
            .bind(attempts as i32)
            .bind(id)
            .execute(&self.write)
            .await
            .map_err(StoreError::backend)?;
        Ok(())
    }
}

/// Flattens a batch DELTA (a fresh `Aggregates` that applied only this batch's
/// events) into `(dimension, bucket, count)` rows for `click_counters`. `total`
/// and `bots` use the empty bucket; every per_* map contributes one row per key.
/// Zero counts are skipped so an all-bot batch never writes empty per_* rows.
fn counter_rows(agg: &Aggregates) -> Vec<(&'static str, String, i64)> {
    let mut rows: Vec<(&'static str, String, i64)> = Vec::new();
    if agg.total > 0 {
        rows.push(("total", String::new(), agg.total as i64));
    }
    if agg.bots > 0 {
        rows.push(("bots", String::new(), agg.bots as i64));
    }
    for (dim, map) in [
        ("day", &agg.per_day),
        ("country", &agg.per_country),
        ("device", &agg.per_device),
        ("os", &agg.per_os),
        ("browser", &agg.per_browser),
        ("referer", &agg.per_referer),
        ("city", &agg.per_city),
        ("variant", &agg.per_variant),
    ] {
        for (bucket, count) in map {
            if *count > 0 {
                rows.push((dim, bucket.clone(), *count as i64));
            }
        }
    }
    rows
}

#[async_trait::async_trait]
impl AnalyticsSink for PostgresStore {
    async fn record_batch(&self, events: &[ClickEvent]) -> Result<(), StoreError> {
        if events.is_empty() {
            return Ok(());
        }
        use std::collections::BTreeMap;
        let mut by_id: BTreeMap<u64, Vec<&ClickEvent>> = BTreeMap::new();
        for e in events {
            by_id.entry(e.id).or_default().push(e);
        }
        let mut tx = self.write.begin().await.map_err(StoreError::backend)?;
        for (id, evs) in by_id {
            let mut delta = Aggregates::default();
            for e in &evs {
                delta.apply(e);
            }
            // All events for the same link share one tenant (the link's owner),
            // so any event in `evs` carries the right value for the whole batch.
            let tenant_id = evs.first().map_or(0, |e| e.tenant_id as i64);
            for (dimension, bucket, count) in counter_rows(&delta) {
                sqlx::query(
                    "INSERT INTO click_counters (id, dimension, bucket, count, tenant_id) VALUES ($1,$2,$3,$4,$5) \
                     ON CONFLICT (id, dimension, bucket) DO UPDATE SET count = click_counters.count + EXCLUDED.count, tenant_id = EXCLUDED.tenant_id",
                )
                .bind(id as i64)
                .bind(dimension)
                .bind(&bucket)
                .bind(count)
                .bind(tenant_id)
                .execute(&mut *tx)
                .await
                .map_err(StoreError::backend)?;
            }
            sqlx::query(
                "INSERT INTO stats_meta (id, first_ts, last_ts, tenant_id) VALUES ($1,$2,$3,$4) \
                 ON CONFLICT (id) DO UPDATE SET first_ts = LEAST(stats_meta.first_ts, EXCLUDED.first_ts), last_ts = GREATEST(stats_meta.last_ts, EXCLUDED.last_ts), tenant_id = EXCLUDED.tenant_id",
            )
            .bind(id as i64)
            .bind(delta.first_ts as i64)
            .bind(delta.last_ts as i64)
            .bind(tenant_id)
            .execute(&mut *tx)
            .await
            .map_err(StoreError::backend)?;
            for e in &evs {
                sqlx::query(
                    "INSERT INTO click_events (id, ts, referer, country, user_agent, city, variant, event_id, tenant_id) \
                     VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
                )
                .bind(id as i64)
                .bind(e.ts as i64)
                .bind(&e.referer)
                .bind(&e.country)
                .bind(&e.user_agent)
                .bind(&e.city)
                .bind(e.variant.map(|v| v as i32))
                .bind(&e.event_id)
                .bind(e.tenant_id as i64)
                .execute(&mut *tx)
                .await
                .map_err(StoreError::backend)?;
            }
            sqlx::query(
                "DELETE FROM click_events WHERE id=$1 AND seq < \
                 (SELECT MIN(seq) FROM (SELECT seq FROM click_events WHERE id=$1 ORDER BY seq DESC LIMIT $2) t)",
            )
            .bind(id as i64)
            .bind(EVENTS_MAX as i64)
            .execute(&mut *tx)
            .await
            .map_err(StoreError::backend)?;
        }
        tx.commit().await.map_err(StoreError::backend)?;
        Ok(())
    }

    async fn stats(&self, id: u64) -> Result<Option<Stats>, StoreError> {
        let counter_rows =
            sqlx::query("SELECT dimension, bucket, count FROM click_counters WHERE id=$1")
                .bind(id as i64)
                .fetch_all(&self.read)
                .await
                .map_err(StoreError::backend)?;
        let mut agg = Aggregates::default();
        for r in &counter_rows {
            let dimension: String = r.try_get("dimension").map_err(StoreError::backend)?;
            let bucket: String = r.try_get("bucket").map_err(StoreError::backend)?;
            let count: i64 = r.try_get("count").map_err(StoreError::backend)?;
            let count = count as u64;
            match dimension.as_str() {
                "total" => agg.total = count,
                "bots" => agg.bots = count,
                "day" => {
                    agg.per_day.insert(bucket, count);
                }
                "country" => {
                    agg.per_country.insert(bucket, count);
                }
                "device" => {
                    agg.per_device.insert(bucket, count);
                }
                "os" => {
                    agg.per_os.insert(bucket, count);
                }
                "browser" => {
                    agg.per_browser.insert(bucket, count);
                }
                "referer" => {
                    agg.per_referer.insert(bucket, count);
                }
                "city" => {
                    agg.per_city.insert(bucket, count);
                }
                "variant" => {
                    agg.per_variant.insert(bucket, count);
                }
                _ => {}
            }
        }
        let meta = sqlx::query("SELECT first_ts, last_ts FROM stats_meta WHERE id=$1")
            .bind(id as i64)
            .fetch_optional(&self.read)
            .await
            .map_err(StoreError::backend)?;
        if let Some(m) = &meta {
            let first_ts: i64 = m.try_get("first_ts").map_err(StoreError::backend)?;
            let last_ts: i64 = m.try_get("last_ts").map_err(StoreError::backend)?;
            agg.first_ts = first_ts as u64;
            agg.last_ts = last_ts as u64;
        }
        let event_rows = sqlx::query(
            "SELECT ts, referer, country, user_agent, city, variant, event_id FROM click_events \
             WHERE id=$1 ORDER BY seq DESC LIMIT $2",
        )
        .bind(id as i64)
        .bind(EVENTS_MAX as i64)
        .fetch_all(&self.read)
        .await
        .map_err(StoreError::backend)?;
        if counter_rows.is_empty() && event_rows.is_empty() {
            return Ok(None);
        }
        let mut recent: Vec<ClickEvent> = Vec::with_capacity(event_rows.len());
        for r in event_rows.iter().rev() {
            let ts: i64 = r.try_get("ts").map_err(StoreError::backend)?;
            let referer: Option<String> = r.try_get("referer").map_err(StoreError::backend)?;
            let country: Option<String> = r.try_get("country").map_err(StoreError::backend)?;
            let user_agent: Option<String> =
                r.try_get("user_agent").map_err(StoreError::backend)?;
            let city: Option<String> = r.try_get("city").map_err(StoreError::backend)?;
            let variant: Option<i32> = r.try_get("variant").map_err(StoreError::backend)?;
            let event_id: String = r.try_get("event_id").map_err(StoreError::backend)?;
            recent.push(ClickEvent {
                id,
                event_id,
                ts: ts as u64,
                referer,
                country,
                bot: is_bot(user_agent.as_deref()),
                user_agent,
                city,
                ip: None,
                fbc: None,
                variant: variant.map(|v| v as u32),
                tenant_id: 0,
            });
        }
        Ok(Some(Stats {
            aggregates: agg,
            recent,
        }))
    }

    /// Same shape as `stats`, but summed across every link owned by
    /// `tenant` instead of keyed by one `id`: `SUM(count) GROUP BY
    /// dimension, bucket` over `click_counters` and `MIN(first_ts)`/
    /// `MAX(last_ts)` over `stats_meta`. No `recent` (per-link raw events
    /// don't compose across links).
    ///
    /// These analytics tables are NOT_FORCED (bare `self.read` pool, no RLS)
    /// — `WHERE tenant_id = $1` on both queries is the ONLY isolation this
    /// aggregate has. Dropping it from either query would leak every
    /// tenant's clicks into every other tenant's `/admin/stats`.
    async fn stats_for_tenant(&self, tenant: u64) -> Result<Aggregates, StoreError> {
        let tenant = tenant as i64;
        let counter_rows = sqlx::query(
            "SELECT dimension, bucket, SUM(count)::BIGINT AS count FROM click_counters \
             WHERE tenant_id = $1 GROUP BY dimension, bucket",
        )
        .bind(tenant)
        .fetch_all(&self.read)
        .await
        .map_err(StoreError::backend)?;
        let mut agg = Aggregates::default();
        for r in &counter_rows {
            let dimension: String = r.try_get("dimension").map_err(StoreError::backend)?;
            let bucket: String = r.try_get("bucket").map_err(StoreError::backend)?;
            let count: i64 = r.try_get("count").map_err(StoreError::backend)?;
            let count = count as u64;
            match dimension.as_str() {
                "total" => agg.total = count,
                "bots" => agg.bots = count,
                "day" => {
                    agg.per_day.insert(bucket, count);
                }
                "country" => {
                    agg.per_country.insert(bucket, count);
                }
                "device" => {
                    agg.per_device.insert(bucket, count);
                }
                "os" => {
                    agg.per_os.insert(bucket, count);
                }
                "browser" => {
                    agg.per_browser.insert(bucket, count);
                }
                "referer" => {
                    agg.per_referer.insert(bucket, count);
                }
                "city" => {
                    agg.per_city.insert(bucket, count);
                }
                "variant" => {
                    agg.per_variant.insert(bucket, count);
                }
                _ => {}
            }
        }
        // A plain aggregate query (no GROUP BY) always returns exactly one
        // row, even over zero matching tenant rows — MIN/MAX just come back
        // NULL, hence the `Option<i64>` binds below rather than `fetch_optional`.
        let meta = sqlx::query(
            "SELECT MIN(first_ts) AS first_ts, MAX(last_ts) AS last_ts FROM stats_meta \
             WHERE tenant_id = $1",
        )
        .bind(tenant)
        .fetch_one(&self.read)
        .await
        .map_err(StoreError::backend)?;
        let first_ts: Option<i64> = meta.try_get("first_ts").map_err(StoreError::backend)?;
        let last_ts: Option<i64> = meta.try_get("last_ts").map_err(StoreError::backend)?;
        agg.first_ts = first_ts.unwrap_or(0) as u64;
        agg.last_ts = last_ts.unwrap_or(0) as u64;
        Ok(agg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_str_round_trips_all_variants() {
        for r in [Role::Owner, Role::Admin, Role::Member, Role::Viewer] {
            assert_eq!(role_from_str(role_to_str(r)).unwrap(), r);
        }
    }

    // Pure constructor check: `PostgresStore` carries the `multi_tenant` flag
    // set by its caller (no DB connection needed — we build the struct
    // literal directly, mirroring what `open`/`open_with_replica` do).
    #[tokio::test]
    async fn multi_tenant_flag_defaults_false_and_is_settable() {
        fn make(multi_tenant: bool) -> PostgresStore {
            PostgresStore {
                write: PgPool::connect_lazy("postgres://unused").unwrap(),
                read: PgPool::connect_lazy("postgres://unused").unwrap(),
                multi_tenant,
                secretbox: None,
            }
        }
        assert!(!make(false).is_multi_tenant());
        assert!(make(true).is_multi_tenant());
    }

    #[test]
    fn needs_reseal_covers_plaintext_v1_and_old_v2_but_not_primary_v2() {
        let primary = "abcd1234";
        // Empty / absent: nothing to do.
        assert!(!needs_reseal(&None, primary));
        assert!(!needs_reseal(&Some(String::new()), primary));
        // Legacy plaintext: reseal.
        assert!(needs_reseal(&Some("plain-secret".to_string()), primary));
        // v1: reseal.
        assert!(needs_reseal(&Some("enc:v1:whatever".to_string()), primary));
        // v2 under a different (rotated-out) key id: reseal.
        assert!(needs_reseal(
            &Some("enc:v2:deadbeef:body".to_string()),
            primary
        ));
        // v2 already under the primary key id: skip.
        assert!(!needs_reseal(
            &Some(format!("enc:v2:{primary}:body")),
            primary
        ));
    }
}
