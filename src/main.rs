use base64::Engine as _;
use quark::analytics::spawn_worker;
use quark::api::{router, AppState};
use quark::cache::valkey::ValkeyTier;
use quark::cache::Cache;
use quark::invalidate::{spawn_invalidation_subscriber, Invalidator, INVALIDATION_CHANNEL};
use quark::store::open_backends;
use quark::webhooks::delivery::{
    spawn_webhook_relay, spawn_webhook_worker, WebhookDispatcher, DELIVERY_TIMEOUT_SECS,
    WEBHOOK_CHANNEL_CAPACITY,
};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// L1 cache capacity (max number of entries held in memory).
const CACHE_CAPACITY: u64 = 100_000;
/// Analytics channel capacity (buffered `ClickEvent`s before backpressure).
const ANALYTICS_CHANNEL_CAPACITY: usize = 10_000;

#[tokio::main]
async fn main() {
    let strict_cluster = std::env::var("QUARK_STRICT_CLUSTER")
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    if let Err(msg) = quark::cluster::cluster_preflight(
        strict_cluster,
        std::env::var("QUARK_DATABASE_URL").is_ok(),
        std::env::var("QUARK_VALKEY_URL").is_ok(),
    ) {
        eprintln!("FATAL: {msg}");
        std::process::exit(1);
    }
    let path = std::env::var("QUARK_DATA").unwrap_or_else(|_| "./data".into());
    let key = std::env::var("QUARK_KEY")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or_else(|| {
            eprintln!("WARNING: QUARK_KEY not set. Using dev key. DO NOT use in production.");
            0x9E3779B97F4A7C15
        });
    // Dedicated secret for signing link-password unlock cookies, separate from
    // QUARK_KEY. From QUARK_SIGNING_KEY (base64, >= 32 bytes) when set; otherwise
    // a random per-process key. A random key means unlock cookies do not survive
    // a restart and are NOT shared across nodes, so multi-node deployments must
    // set QUARK_SIGNING_KEY.
    let signing_key: [u8; 32] = std::env::var("QUARK_SIGNING_KEY")
        .ok()
        .and_then(|s| {
            base64::engine::general_purpose::STANDARD
                .decode(s.trim())
                .ok()
        })
        .filter(|b| b.len() >= 32)
        .map(|b| {
            let mut k = [0u8; 32];
            k.copy_from_slice(&b[..32]);
            k
        })
        .unwrap_or_else(|| {
            eprintln!(
                "WARNING: QUARK_SIGNING_KEY not set (or < 32 bytes). Using a random per-process \
                 key; link-password unlock cookies will not survive a restart and are not shared \
                 across nodes. Set QUARK_SIGNING_KEY for multi-node or persistent deployments."
            );
            let mut k = [0u8; 32];
            getrandom::fill(&mut k).expect("system RNG must be available");
            k
        });
    let multi_tenant = std::env::var("QUARK_MULTI_TENANT")
        .map(|v| v != "0")
        .unwrap_or(false);
    if multi_tenant {
        eprintln!("multi-tenant mode ENABLED (FORCE RLS + per-tenant tx on Postgres)");
    }
    // Base suffix for the auto per-tenant subdomain (multi-tenancy
    // P3-completion), e.g. `quarkus.com.br`. Cloud-only; unset disables the
    // whole feature (no seed on tenant creation, no boot backfill below).
    let tenant_domain_suffix = std::env::var("QUARK_TENANT_DOMAIN_SUFFIX")
        .ok()
        .map(|s| s.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|s| !s.is_empty());
    let (store, sink) = open_backends(std::path::Path::new(&path), multi_tenant)
        .await
        .expect("open backends");
    eprintln!(
        "backend: {}",
        if std::env::var("QUARK_DATABASE_URL").is_ok() {
            "postgres"
        } else {
            "lmdb"
        }
    );
    eprintln!(
        "analytics sink: {}",
        if std::env::var("QUARK_CLICKHOUSE_URL").is_ok() {
            "clickhouse"
        } else if std::env::var("QUARK_DATABASE_URL").is_ok() {
            "postgres"
        } else {
            "lmdb(embedded)"
        }
    );

    // Secret-at-rest re-encryption boot backfill (LUC-48 Task 3): once
    // QUARK_ENCRYPTION_KEY is turned on, every legacy plaintext oidc
    // client_secret / sheets refresh_token gets sealed. Idempotent (already-
    // sealed rows are skipped) and a no-op on every other backend/config —
    // safe to run on every replica, every boot.
    if std::env::var("QUARK_ENCRYPTION_KEY").is_ok() {
        match store.reencrypt_legacy_secrets().await {
            Ok(n) => eprintln!("secret re-encryption backfill: {n} re-encrypted"),
            Err(e) => eprintln!("WARNING: secret re-encryption backfill failed: {e}"),
        }
    }

    // Auto per-tenant subdomain boot backfill (multi-tenancy P3-completion):
    // every existing tenant gets its `<slug>.<suffix>` `domains` row, same as
    // a freshly created one (`admin_tenants_create`). Idempotent (skips
    // tenants that already have the row) and cheap (few tenants, once per
    // boot) — safe to run on every replica.
    if multi_tenant {
        if let Some(suffix) = &tenant_domain_suffix {
            match store.list_tenants().await {
                Ok(tenants) => {
                    let mut seeded = 0usize;
                    for t in &tenants {
                        let host = quark::api::subdomain_host(&t.slug, suffix);
                        match store.get_domain_by_host(&host).await {
                            Ok(Some(_)) => {} // already seeded
                            Ok(None) => {
                                match quark::api::seed_tenant_subdomain(
                                    &store, t.id, &t.slug, suffix,
                                )
                                .await
                                {
                                    Ok(()) => seeded += 1,
                                    Err(e) => eprintln!(
                                        "{}",
                                        serde_json::json!({ "tenant_subdomain_backfill_error": e.to_string(), "tenant_id": t.id.0 })
                                    ),
                                }
                            }
                            Err(e) => eprintln!(
                                "{}",
                                serde_json::json!({ "tenant_subdomain_backfill_error": e.to_string(), "tenant_id": t.id.0 })
                            ),
                        }
                    }
                    eprintln!(
                        "tenant subdomain backfill: {seeded} seeded, {} already present (suffix {suffix})",
                        tenants.len() - seeded
                    );
                }
                Err(e) => eprintln!(
                    "WARNING: tenant subdomain backfill skipped (list_tenants failed: {e})"
                ),
            }
        }
    }
    match std::env::var("QUARK_NODE_ID") {
        Ok(n) if !n.is_empty() && std::env::var("QUARK_DATABASE_URL").is_ok() => {
            eprintln!(
                "WARNING: QUARK_NODE_ID={n} ignored on the Postgres backend (node-id is LMDB-only)"
            );
        }
        Ok(n) if !n.is_empty() => {
            eprintln!("========================================================================");
            eprintln!(
                "WARNING: QUARK_NODE_ID={n} set on the LMDB backend (no QUARK_DATABASE_URL)."
            );
            eprintln!("  LMDB stores are per-node: each replica keeps its OWN file and replicas");
            eprintln!("  do NOT share links. A redirect that lands on a node without the link");
            eprintln!("  returns 404. node-id only partitions the id space (8+32 bits) so codes");
            eprintln!("  do not collide; it does NOT make this a shared multi-node store.");
            eprintln!("  True multi-node needs the Postgres backend (set QUARK_DATABASE_URL).");
            eprintln!("  The node id MUST be unique per replica (e.g. a StatefulSet ordinal);");
            eprintln!("  quark cannot detect a duplicate and a collision silently reuses ids.");
            eprintln!("========================================================================");
        }
        _ => {}
    }
    let control_conn: Option<redis::aio::MultiplexedConnection> =
        match std::env::var("QUARK_VALKEY_URL").ok() {
            Some(url) => match redis::Client::open(url) {
                Ok(client) => client.get_multiplexed_async_connection().await.ok(),
                Err(_) => None,
            },
            None => None,
        };
    let invalidator: Option<Arc<Invalidator>> = control_conn
        .clone()
        .map(|conn| Arc::new(Invalidator { conn: Some(conn) }));

    let cache = match std::env::var("QUARK_VALKEY_URL").ok() {
        Some(url) => {
            match ValkeyTier::open(&url).await {
                Ok(tier) => {
                    let shown = url.rsplit('@').next().unwrap_or(&url);
                    eprintln!("L2 Valkey enabled: {shown}");
                    Cache::with_l2(
                        store.clone(),
                        CACHE_CAPACITY,
                        Arc::new(tier),
                        quark::cache::L1_TTL_SECS,
                        quark::cache::L2_TTL_SECS,
                        invalidator.clone(),
                    )
                }
                Err(e) => {
                    eprintln!("WARNING: failed to connect to Valkey ({e}); continuing with L1+store only.");
                    Cache::new(store.clone(), CACHE_CAPACITY, invalidator.clone())
                }
            }
        }
        None => Cache::new(store.clone(), CACHE_CAPACITY, invalidator.clone()),
    };
    let (analytics_tx, analytics_rx) = tokio::sync::mpsc::channel(ANALYTICS_CHANNEL_CAPACITY);
    let pixel_client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .expect("build pixel forwarding client");
    let _worker = spawn_worker(
        analytics_rx,
        sink.clone(),
        store.clone(),
        pixel_client,
        key,
        quark::pixel::PixelBases::default(),
    );
    let admin_token = std::env::var("QUARK_ADMIN_TOKEN").ok();
    if admin_token.is_none() {
        eprintln!("WARNING: QUARK_ADMIN_TOKEN not set — /stats endpoint disabled.");
    }
    if std::env::var("QUARK_ACCESS_LOG").is_err() {
        eprintln!("per-request access log disabled (set QUARK_ACCESS_LOG=1 to enable)");
    }

    let per_min: u32 = std::env::var("QUARK_RATELIMIT_PER_MIN")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let ratelimiter = match (per_min, control_conn.clone()) {
        (0, _) => quark::abuse::ratelimit::RateLimiter::disabled(),
        (n, Some(conn)) => quark::abuse::ratelimit::RateLimiter::valkey(n, conn),
        (n, None) => quark::abuse::ratelimit::RateLimiter::memory(n),
    };
    if per_min == 0 {
        eprintln!("rate-limit disabled (set QUARK_RATELIMIT_PER_MIN=n to enable)");
    } else {
        eprintln!(
            "rate-limit: {per_min}/min per IP ({})",
            if control_conn.is_some() {
                "global via Valkey"
            } else {
                "per replica (memory)"
            }
        );
    }
    let block_private = std::env::var("QUARK_BLOCK_PRIVATE")
        .map(|v| v != "0")
        .unwrap_or(true);
    // Lowercased so the self-loop / claim-prevention checks that compare an
    // incoming host (always lowercased) against it can never be bypassed by a
    // mixed-case env value.
    let public_host = std::env::var("QUARK_PUBLIC_HOST")
        .ok()
        .map(|h| h.trim().trim_end_matches('.').to_ascii_lowercase());
    let real_ip_header =
        std::env::var("QUARK_REAL_IP_HEADER").unwrap_or_else(|_| "cf-connecting-ip".to_string());

    let (wh_tx, wh_rx) = tokio::sync::mpsc::channel(WEBHOOK_CHANNEL_CAPACITY);
    let clicked = Arc::new(AtomicBool::new(false));
    let expired = Arc::new(AtomicBool::new(false));
    spawn_webhook_worker(wh_rx, store.clone(), clicked.clone(), expired.clone());
    let dispatcher = WebhookDispatcher::new(wh_tx, clicked, expired);
    let webhooks = if std::env::var("QUARK_DATABASE_URL").is_ok() {
        let relay_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(DELIVERY_TIMEOUT_SECS))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("build webhook relay client");
        spawn_webhook_relay(store.clone(), relay_client);
        eprintln!(
            "webhook delivery: durable Postgres outbox + leased relay (lifecycle events); clicked/expired best-effort in-memory"
        );
        Arc::new(dispatcher.with_outbox(store.clone()))
    } else {
        eprintln!("webhook delivery: in-memory best-effort channel (LMDB backend)");
        Arc::new(dispatcher)
    };

    // OIDC login (opt-in via QUARK_OIDC_ISSUER). A failed init disables login but
    // never blocks startup: the break-glass admin token still works.
    let oidc_config = quark::oidc::OidcConfig::from_env();
    let oidc_configured = oidc_config.is_some();
    let oidc = match oidc_config {
        Some(cfg) => {
            let issuer = cfg.issuer.clone();
            match quark::oidc::OidcRuntime::init(cfg).await {
                Ok(rt) => {
                    eprintln!("oidc login: enabled (issuer {issuer})");
                    Some(Arc::new(rt))
                }
                Err(e) => {
                    eprintln!("WARNING: OIDC configured but init failed ({e}); login disabled, admin token still works");
                    None
                }
            }
        }
        None => {
            eprintln!("oidc login: disabled (set QUARK_OIDC_ISSUER to enable)");
            None
        }
    };

    // Google Sheets connector (opt-in via QUARK_SHEETS_CLIENT_ID/_SECRET/
    // _REDIRECT_URL). The HTTP seam is always built; the config gates whether the
    // routes and the scheduled sync do anything.
    let sheets_config = quark::sheets::SheetsConfig::from_env();
    let sheets_api: std::sync::Arc<dyn quark::sheets::client::SheetsApi> =
        std::sync::Arc::new(quark::sheets::client::GoogleSheetsApi {
            client: quark::sheets::client::http_client(),
        });
    match &sheets_config {
        Some(cfg) => match cfg.sync_secs {
            Some(secs) => eprintln!("sheets sync: enabled (scheduled every {secs}s)"),
            None => eprintln!("sheets sync: enabled (on demand)"),
        },
        None => eprintln!(
            "sheets sync: disabled (set QUARK_SHEETS_CLIENT_ID/_SECRET/_REDIRECT_URL to enable)"
        ),
    }
    let sheets = sheets_config.map(Arc::new);

    // Custom-domain host routing (multi-tenancy P3). Meaningful only in cloud
    // (`multi_tenant`); in OSS every host still resolves through `public_host`
    // to the shared route, since no domain is ever `Verified` there.
    let host_router = Arc::new(quark::domain_router::HostRouter::new(
        store.clone(),
        public_host.clone(),
        None,
    ));
    // DNS is used only by custom-domain / SSO-domain TXT verification, both
    // cloud-only. In OSS (`!multi_tenant`) nothing calls it, so skip building
    // the system resolver and use the inert `NullDns` (LUC-51).
    let dns: Arc<dyn quark::dns::Dns> = if multi_tenant {
        Arc::new(quark::dns::HickoryDns::new().expect("failed to build DNS resolver"))
    } else {
        Arc::new(quark::dns::NullDns)
    };

    // Keycloak-hosted auth (multi-tenancy P2e, opt-in via
    // QUARK_KEYCLOAK_BASE_URL). Foundation only here: the trait + HTTP client +
    // config. The provisioning flow that calls it on tenant creation is a
    // later task.
    let keycloak_config = quark::keycloak::KeycloakConfig::from_env();
    let keycloak_base_url = keycloak_config.as_ref().map(|c| c.base_url.clone());
    let keycloak: Option<Arc<dyn quark::keycloak::KeycloakAdmin>> = match keycloak_config {
        Some(cfg) => {
            let base = cfg.base_url.clone();
            eprintln!("keycloak admin: enabled (base {base})");
            Some(Arc::new(quark::keycloak::client::HttpKeycloakAdmin::new(
                cfg,
                quark::keycloak::client::keycloak_client(),
            )))
        }
        None => {
            eprintln!("keycloak admin: disabled (set QUARK_KEYCLOAK_BASE_URL to enable)");
            None
        }
    };

    // Keycloak tenant provisioning boot backfill (multi-tenancy P2e Task 2):
    // every tenant that has no `oidc_config` yet (created before Keycloak was
    // configured, or whose creation-time attempt only got partway) gets
    // (re-)provisioned here. Idempotent and cheap, like the subdomain
    // backfill above — safe to run on every replica.
    if multi_tenant {
        if let (Some(kc), Some(base)) = (&keycloak, &keycloak_base_url) {
            match quark::api::backfill_keycloak_provisioning(&store, kc, base).await {
                Ok(n) => eprintln!("keycloak tenant backfill: {n} provisioned"),
                Err(e) => eprintln!(
                    "WARNING: keycloak tenant backfill skipped (list_tenants failed: {e})"
                ),
            }
        }
    }

    let state = Arc::new(AppState {
        cache,
        store,
        key,
        signing_key,
        analytics_tx,
        sink,
        admin_token,
        ratelimiter,
        block_private,
        public_host,
        real_ip_header,
        webhooks,
        oidc,
        oidc_configured,
        sheets,
        sheets_api: Some(sheets_api),
        multi_tenant,
        host_router,
        dns,
        tenant_domain_suffix,
        oidc_tenants: quark::oidc::TenantOidcCache::new(),
        keycloak,
        keycloak_base_url,
    });
    match std::env::var("QUARK_VALKEY_URL").ok() {
        Some(url) => {
            eprintln!("cross-node invalidation: pub/sub subscriber on {INVALIDATION_CHANNEL}");
            let _sub = spawn_invalidation_subscriber(url, state.clone());
        }
        None => eprintln!("cross-node invalidation: disabled (no QUARK_VALKEY_URL)"),
    }

    // Broken-link monitoring (opt-in). Runs when QUARK_HEALTH_CHECK_SECS is set.
    // Safe to enable on every replica: a lease (Postgres) ensures only one node
    // sweeps at a time, renewed during the sweep, with automatic failover if the
    // holder dies. On the single-node LMDB backend the lease is always granted.
    match std::env::var("QUARK_HEALTH_CHECK_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
    {
        Some(secs) => {
            let period = std::time::Duration::from_secs(secs.max(quark::health::MIN_CHECK_SECS));
            let _checker = quark::health::spawn_link_checker(
                state.store.clone(),
                state.webhooks.clone(),
                quark::health::build_client(),
                period,
                state.key,
            );
            eprintln!(
                "link health checker: sweeping every {}s (lease-coordinated; safe on all replicas)",
                period.as_secs()
            );
        }
        None => eprintln!("link health checker: disabled (set QUARK_HEALTH_CHECK_SECS to enable)"),
    }

    // Scheduled Sheets sync (opt-in via QUARK_SHEETS_SYNC_SECS). Lease-coordinated
    // like the link checker so it is safe on every replica; on the single-node
    // LMDB backend the lease is always granted. Never logs the token.
    if let (Some(cfg), Some(api)) = (state.sheets.clone(), state.sheets_api.clone()) {
        // The scheduled sync has no request to read a Host from, so it needs
        // QUARK_PUBLIC_HOST to build correct short URLs. Without it, skip the
        // schedule (on-demand sync still works, using the request Host) rather
        // than write "https://localhost/<code>" links into the sheet.
        if cfg.sync_secs.is_some() && state.public_host.is_none() {
            eprintln!(
                "sheets sync: scheduled sync disabled (set QUARK_PUBLIC_HOST so short URLs are correct; on-demand sync still works)"
            );
        }
        if let (Some(secs), Some(public_host)) = (cfg.sync_secs, state.public_host.clone()) {
            let store = state.store.clone();
            let key = state.key;
            let base_url = format!("https://{public_host}");
            // Per-process holder id for the sync lease (mirrors the health checker).
            let mut hb = [0u8; 8];
            let _ = getrandom::fill(&mut hb);
            let holder: String = format!(
                "sheets_{}",
                hb.iter().map(|b| format!("{b:02x}")).collect::<String>()
            );
            let ttl = secs.saturating_mul(2);
            tokio::spawn(async move {
                let client = quark::sheets::client::http_client();
                let mut ticker = tokio::time::interval(std::time::Duration::from_secs(secs));
                loop {
                    ticker.tick().await;
                    if !store
                        .try_acquire_sheets_lease(&holder, ttl)
                        .await
                        .unwrap_or(false)
                    {
                        continue;
                    }
                    let Ok(Some(mut conn)) = store
                        .get_sheets_connection(quark::tenant::DEFAULT_TENANT)
                        .await
                    else {
                        continue;
                    };
                    let outcome = match quark::sheets::refresh_access_token(
                        &client,
                        &cfg,
                        &conn.refresh_token,
                    )
                    .await
                    {
                        Ok(token) => {
                            quark::sheets::sync(
                                &store,
                                api.as_ref(),
                                key,
                                &base_url,
                                &mut conn,
                                &token,
                                quark::now(),
                            )
                            .await
                        }
                        Err(e) => Err(e),
                    };
                    if let Err(e) = &outcome {
                        conn.last_status = quark::sheets::SyncStatus::Error(e.clone());
                        eprintln!("{}", serde_json::json!({ "sheets_sync_error": e }));
                    } else {
                        eprintln!("{}", serde_json::json!({ "sheets_sync": "ok" }));
                    }
                    if let Err(e) = store
                        .put_sheets_connection(quark::tenant::DEFAULT_TENANT, &conn)
                        .await
                    {
                        eprintln!(
                            "{}",
                            serde_json::json!({ "sheets_sync_persist_error": e.to_string() })
                        );
                    }
                }
            });
        }
    }

    // Garbage-collect expired OIDC login sessions hourly (only when OIDC is on).
    if state.oidc.is_some() {
        let store = state.store.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(3600));
            loop {
                ticker.tick().await;
                if let Err(e) = store.gc_sessions(quark::now()).await {
                    eprintln!(
                        "{}",
                        serde_json::json!({ "session_gc_error": e.to_string() })
                    );
                }
            }
        });
    }

    let app = router(state);

    let addr = std::env::var("QUARK_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    eprintln!("quark listening on {addr}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await
    .expect("serve");
}
