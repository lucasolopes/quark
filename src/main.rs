use anyhow::Context as _;
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
/// Request budget for forwarding a conversion to a third-party pixel. Short
/// because the forwarding worker shares the runtime with the redirect path and
/// the delivery is best-effort anyway.
const PIXEL_FORWARD_TIMEOUT_SECS: u64 = 5;
/// Cadence of the two hourly housekeeping loops (expired OIDC session GC and
/// the analytics retention purge). Both only reclaim storage, so an hour is
/// frequent enough and keeps the load off the request path.
const HOUSEKEEPING_INTERVAL_SECS: u64 = 3600;
/// Ceiling for the scheduled Sheets sync lease TTL. The lease is a crash-safety
/// cap only (the tick releases it when done), so it is capped low enough that
/// even a missed release frees it before the next tick.
const SHEETS_LEASE_TTL_CAP_SECS: u64 = 300;

/// Analytics retention window (LUC-65, GDPR), in seconds, from the raw
/// `QUARK_ANALYTICS_RETENTION_DAYS` env value plus the `multi_tenant` mode.
///
/// - Env absent: mode default (cloud/`multi_tenant` = 365 days, OSS/self-host
///   = unlimited).
/// - Env set and a valid `u64` day count: that value wins (`0` means "purge
///   everything", i.e. keep no history; this is intentional, not a bug).
/// - Env set but NOT a valid `u64` (typo, negative, non-numeric, ...): this is
///   a compliance footgun if silently treated as "unlimited" in cloud, so it
///   falls back to the mode default instead of disabling retention. Call
///   sites should log a warning when this happens (this fn stays pure/log-free
///   so it's easy to unit test).
fn retention_secs_from(env_value: Option<&str>, multi_tenant: bool) -> Option<u64> {
    let mode_default = || {
        if multi_tenant {
            Some(365 * 86_400)
        } else {
            None
        }
    };
    match env_value {
        Some(v) => match v.trim().parse::<u64>() {
            Ok(days) => Some(days * 86_400),
            Err(_) => mode_default(),
        },
        None => mode_default(),
    }
}

/// Installs the tracing subscriber. `RUST_LOG` drives the filter (the ecosystem
/// standard, so `tokio`, `sqlx`, `reqwest`, `hickory-resolver` and `moka` events
/// are reachable too); `QUARK_LOG_FORMAT=json` switches the output from the
/// human-readable format to one JSON object per event, which is what the
/// production log pipeline wants.
///
/// The per-request access log now comes from tower-http's `TraceLayer` at DEBUG,
/// so it is enabled with `RUST_LOG=tower_http=debug` instead of a dedicated env
/// var.
fn init_tracing() {
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let registry = tracing_subscriber::registry().with(filter);
    if std::env::var("QUARK_LOG_FORMAT").as_deref() == Ok("json") {
        registry
            .with(tracing_subscriber::fmt::layer().json())
            .init();
    } else {
        registry.with(tracing_subscriber::fmt::layer()).init();
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // `--version` answers before anything else is set up: it must work with no
    // config, no backends, and no log subscriber installed. Stays a `println!`
    // because it is program output on stdout, not a log line.
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("quark {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    init_tracing();
    let strict_cluster = std::env::var("QUARK_STRICT_CLUSTER")
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    quark::cluster::cluster_preflight(
        strict_cluster,
        std::env::var("QUARK_DATABASE_URL").is_ok(),
        std::env::var("QUARK_VALKEY_URL").is_ok(),
    )
    .context("cluster preflight")?;
    let path = std::env::var("QUARK_DATA").unwrap_or_else(|_| "./data".into());
    let key = std::env::var("QUARK_KEY")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or_else(|| {
            tracing::warn!("QUARK_KEY not set. Using dev key. DO NOT use in production.");
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
            tracing::warn!(
                "QUARK_SIGNING_KEY not set (or < 32 bytes). Using a random per-process \
                 key; link-password unlock cookies will not survive a restart and are not shared \
                 across nodes. Set QUARK_SIGNING_KEY for multi-node or persistent deployments."
            );
            let mut k = [0u8; 32];
            #[expect(clippy::expect_used, reason = "the OS RNG being unavailable is not a recoverable condition for a security path")]
            getrandom::fill(&mut k).expect("system RNG must be available");
            k
        });
    let multi_tenant = std::env::var("QUARK_MULTI_TENANT")
        .map(|v| v != "0")
        .unwrap_or(false);
    if multi_tenant {
        tracing::info!("multi-tenant mode ENABLED (FORCE RLS + per-tenant tx on Postgres)");
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
        .context("opening the storage backends")?;
    // Boot lines carry the selected backend as a field, not inside the message:
    // counting deployments per backend is a legitimate query.
    tracing::info!(
        backend = if std::env::var("QUARK_DATABASE_URL").is_ok() {
            "postgres"
        } else {
            "lmdb"
        },
        "store backend selected"
    );
    tracing::info!(
        sink = if std::env::var("QUARK_CLICKHOUSE_URL").is_ok() {
            "clickhouse"
        } else if std::env::var("QUARK_DATABASE_URL").is_ok() {
            "postgres"
        } else {
            "lmdb_embedded"
        },
        "analytics sink selected"
    );

    // Secret-at-rest re-encryption boot backfill (LUC-48 Task 3): once
    // QUARK_ENCRYPTION_KEY is turned on, every legacy plaintext oidc
    // client_secret / sheets refresh_token gets sealed. Idempotent (already-
    // sealed rows are skipped) and a no-op on every other backend/config —
    // safe to run on every replica, every boot.
    if std::env::var("QUARK_ENCRYPTION_KEY").is_ok() {
        match store.reencrypt_legacy_secrets().await {
            Ok(n) => {
                tracing::info!(re_encrypted = n, "secret re-encryption backfill completed")
            }
            Err(e) => tracing::warn!(error = %e, "secret re-encryption backfill failed"),
        }
    }
    match std::env::var("QUARK_NODE_ID") {
        Ok(n) if !n.is_empty() && std::env::var("QUARK_DATABASE_URL").is_ok() => {
            tracing::warn!(
                node_id = %n,
                "QUARK_NODE_ID ignored on the Postgres backend (node-id is LMDB-only)"
            );
        }
        // One event, not a ten-line banner: the explanation used to be split
        // across `info!` lines around a single `warn!`, so anything filtering
        // at WARN got the alarm without the reason.
        Ok(n) if !n.is_empty() => {
            tracing::warn!(
                node_id = %n,
                "QUARK_NODE_ID set on the LMDB backend (no QUARK_DATABASE_URL). LMDB stores are \
                 per-node: each replica keeps its OWN file and replicas do NOT share links. A \
                 redirect that lands on a node without the link returns 404. node-id only \
                 partitions the id space (8+32 bits) so codes do not collide; it does NOT make \
                 this a shared multi-node store. True multi-node needs the Postgres backend (set \
                 QUARK_DATABASE_URL). The node id MUST be unique per replica (e.g. a StatefulSet \
                 ordinal); quark cannot detect a duplicate and a collision silently reuses ids."
            );
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
        Some(url) => match ValkeyTier::open(&url).await {
            Ok(tier) => {
                let shown = url.rsplit('@').next().unwrap_or(&url);
                tracing::info!(endpoint = %shown, "L2 Valkey cache enabled");
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
                tracing::warn!(
                    error = %e,
                    "could not connect to Valkey, continuing with L1 and store only"
                );
                Cache::new(store.clone(), CACHE_CAPACITY, invalidator.clone())
            }
        },
        None => Cache::new(store.clone(), CACHE_CAPACITY, invalidator.clone()),
    };
    let (analytics_tx, analytics_rx) = tokio::sync::mpsc::channel(ANALYTICS_CHANNEL_CAPACITY);
    let pixel_client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(PIXEL_FORWARD_TIMEOUT_SECS))
        .connect_timeout(std::time::Duration::from_secs(
            quark::HTTP_CONNECT_TIMEOUT_SECS,
        ))
        .build()
        .context("building the pixel forwarding client")?;
    let admin_token = std::env::var("QUARK_ADMIN_TOKEN").ok();
    if admin_token.is_none() {
        tracing::warn!("QUARK_ADMIN_TOKEN not set — /stats endpoint disabled.");
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
        tracing::info!("rate-limit disabled (set QUARK_RATELIMIT_PER_MIN=n to enable)");
    } else {
        tracing::info!(
            per_min,
            scope = if control_conn.is_some() {
                "global_valkey"
            } else {
                "per_replica_memory"
            },
            "rate-limit enabled per IP"
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
    let real_ip_header = std::env::var("QUARK_REAL_IP_HEADER")
        .unwrap_or_else(|_| quark::api::DEFAULT_REAL_IP_HEADER.to_string());

    let (wh_tx, wh_rx) = tokio::sync::mpsc::channel(WEBHOOK_CHANNEL_CAPACITY);
    let clicked = Arc::new(AtomicBool::new(false));
    let expired = Arc::new(AtomicBool::new(false));
    let webhook_worker =
        spawn_webhook_worker(wh_rx, store.clone(), clicked.clone(), expired.clone());
    let dispatcher = WebhookDispatcher::new(wh_tx, clicked, expired);
    let webhooks = if std::env::var("QUARK_DATABASE_URL").is_ok() {
        let relay_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(DELIVERY_TIMEOUT_SECS))
            .connect_timeout(std::time::Duration::from_secs(
                quark::HTTP_CONNECT_TIMEOUT_SECS,
            ))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("building the webhook relay client")?;
        spawn_webhook_relay(store.clone(), relay_client);
        tracing::info!(
            "webhook delivery: durable Postgres outbox + leased relay (lifecycle events); clicked/expired best-effort in-memory"
        );
        Arc::new(dispatcher.with_outbox(store.clone()))
    } else {
        tracing::info!("webhook delivery: in-memory best-effort channel (LMDB backend)");
        Arc::new(dispatcher)
    };

    // Analytics worker (off the redirect hot path): batches clicks to the sink,
    // forwards to pixels, and drives the click-threshold alert engine (LUC-38),
    // emitting `link.threshold_reached` through `webhooks`. `control_conn` is
    // the shared Valkey counter (`None` -> per-replica in-memory counting).
    // Opt-in IP anonymization for the Meta forward (LUC-65, GDPR): when on,
    // `client_ip_address` is truncated (IPv4 /24, IPv6 /48) before it leaves
    // the instance. Default off keeps the pre-LUC-65 behavior (raw IP).
    let pixel_anonymize_ip = std::env::var("QUARK_PIXEL_ANONYMIZE_IP")
        .map(|v| v != "0")
        .unwrap_or(false);
    if pixel_anonymize_ip {
        tracing::info!("pixel forward: IP anonymization ENABLED (IPv4 /24, IPv6 /48)");
    }
    let pixel_bases = quark::pixel::PixelBases {
        anonymize_ip: pixel_anonymize_ip,
        ..quark::pixel::PixelBases::default()
    };
    let analytics_worker = spawn_worker(
        analytics_rx,
        sink.clone(),
        store.clone(),
        pixel_client,
        key,
        pixel_bases,
        webhooks.clone(),
        control_conn.clone(),
    );

    // OIDC login (opt-in via QUARK_OIDC_ISSUER). A failed init disables login but
    // never blocks startup: the break-glass admin token still works.
    let oidc_config = quark::oidc::OidcConfig::from_env();
    let oidc_configured = oidc_config.is_some();
    let oidc = match oidc_config {
        Some(cfg) => {
            let issuer = cfg.issuer.clone();
            match quark::oidc::OidcRuntime::init(cfg).await {
                Ok(rt) => {
                    tracing::info!(issuer = %issuer, "oidc login enabled");
                    Some(Arc::new(rt))
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "OIDC configured but init failed, login disabled and the admin token \
                         still works"
                    );
                    None
                }
            }
        }
        None => {
            tracing::info!("oidc login: disabled (set QUARK_OIDC_ISSUER to enable)");
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
            // `scheduled` is on both arms so a pipeline can tell the two modes
            // apart by field. With it only on one, the absent field and a
            // disabled sync look the same.
            Some(secs) => tracing::info!(
                scheduled = true,
                sync_secs = secs,
                "sheets sync enabled, scheduled"
            ),
            None => tracing::info!(scheduled = false, "sheets sync enabled, on demand"),
        },
        None => tracing::info!(
            "sheets sync: disabled (set QUARK_SHEETS_CLIENT_ID/_SECRET/_REDIRECT_URL to enable)"
        ),
    }
    let sheets = sheets_config.map(Arc::new);

    // Slack "Add to Slack" connector (opt-in via QUARK_SLACK_CLIENT_ID/_SECRET/
    // _REDIRECT_URL). The OAuth install persists a `kind: Slack` webhook
    // subscription; no bespoke storage or HTTP seam is needed.
    let slack = quark::slack::SlackConfig::from_env();
    match &slack {
        Some(_) => tracing::info!("slack connect: enabled (Add to Slack OAuth)"),
        None => tracing::info!(
            "slack connect: disabled (set QUARK_SLACK_CLIENT_ID/_SECRET/_REDIRECT_URL to enable)"
        ),
    }
    let slack = slack.map(Arc::new);

    // Custom-domain host routing (multi-tenancy P3). Meaningful only in cloud
    // (`multi_tenant`); in OSS every host still resolves through `public_host`
    // to the shared route, since no domain is ever `Verified` there.
    let host_router = Arc::new(quark::domain_router::HostRouter::new(
        store.clone(),
        public_host.clone(),
        invalidator.clone(),
    ));
    // DNS is used only by custom-domain / SSO-domain TXT verification, both
    // cloud-only. In OSS (`!multi_tenant`) nothing calls it, so skip building
    // the system resolver and use the inert `NullDns` (LUC-51).
    let dns: Arc<dyn quark::dns::Dns> = if multi_tenant {
        Arc::new(quark::dns::HickoryDns::new().context("building the DNS resolver")?)
    } else {
        Arc::new(quark::dns::NullDns)
    };
    // Boot da edicao Enterprise (LUC-19): runtime do Keycloak, backfill de
    // provisionamento e seed do subdominio automatico de cada tenant. Tudo isso
    // vive em `src/ee/`, que nao e AGPL e so entra no binario com
    // `--features ee`. Uma chamada so, e nao `cfg` espalhado pelo boot.
    #[cfg(feature = "ee")]
    let ee = quark::ee::boot(&store, multi_tenant, tenant_domain_suffix.as_deref()).await;

    let state = Arc::new(AppState {
        cache,
        store,
        key,
        signing_key: secrecy::SecretBox::new(Box::new(signing_key)),
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
        slack,
        multi_tenant,
        host_router,
        dns,
        tenant_domain_suffix,
        oidc_tenants: quark::oidc::TenantOidcCache::new(),
        #[cfg(feature = "ee")]
        ee,
    });
    let invalidation_sub = match std::env::var("QUARK_VALKEY_URL").ok() {
        Some(url) => {
            tracing::info!(
                channel = INVALIDATION_CHANNEL,
                "cross-node invalidation pub/sub subscriber started"
            );
            Some(spawn_invalidation_subscriber(url, state.clone()))
        }
        None => {
            tracing::info!("cross-node invalidation: disabled (no QUARK_VALKEY_URL)");
            None
        }
    };

    // Broken-link monitoring (opt-in). Runs when QUARK_HEALTH_CHECK_SECS is set.
    // Safe to enable on every replica: a lease (Postgres) ensures only one node
    // sweeps at a time, renewed during the sweep, with automatic failover if the
    // holder dies. On the single-node LMDB backend the lease is always granted.
    let link_checker = match std::env::var("QUARK_HEALTH_CHECK_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
    {
        Some(secs) => {
            let period = std::time::Duration::from_secs(secs.max(quark::health::MIN_CHECK_SECS));
            let checker = quark::health::spawn_link_checker(
                state.store.clone(),
                state.webhooks.clone(),
                quark::health::build_client(),
                period,
                state.key,
            );
            tracing::info!(
                period_secs = period.as_secs(),
                "link health checker enabled, lease-coordinated and safe on all replicas"
            );
            Some(checker)
        }
        None => {
            tracing::info!("link health checker: disabled (set QUARK_HEALTH_CHECK_SECS to enable)");
            None
        }
    };

    spawn_sheets_sync(&state);

    spawn_session_gc(&state);

    spawn_analytics_purge(&state, multi_tenant);

    let app = router(state);

    let addr = std::env::var("QUARK_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    tracing::info!(addr = %addr, "quark listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .context("running the HTTP server")?;

    drain_workers(
        analytics_worker,
        webhook_worker,
        invalidation_sub,
        link_checker,
    )
    .await;

    Ok(())
}

/// Resolves on SIGTERM (what a Fly or Docker deploy sends) or Ctrl-C.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            // Without SIGTERM we still honor Ctrl-C; never fail the process here.
            Err(e) => {
                tracing::warn!(error = %e, "could not install the SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
    tracing::info!("shutdown signal received, draining");
}

/// How long the drain may take before the process exits anyway. A deploy must
/// not hang on a wedged worker.
const DRAIN_TIMEOUT_SECS: u64 = 10;

/// Drains the background workers after the HTTP server stopped accepting.
///
/// Order matters. The analytics and webhook channels close when the last
/// `Arc<AppState>` drops, because that is where their `Sender`s live: the
/// server owned one and released it above, and the invalidation subscriber
/// holds the only other clone. So the cache-only tasks are cancelled and joined
/// FIRST, which releases that clone; only then do the two workers observe the
/// channel close, flush what they buffered, and exit.
///
/// Everything is bounded by `DRAIN_TIMEOUT_SECS`: losing a few buffered events
/// is better than a deploy that never finishes.
async fn drain_workers(
    analytics: tokio::task::JoinHandle<()>,
    webhooks: tokio::task::JoinHandle<()>,
    invalidation: Option<tokio::task::JoinHandle<()>>,
    link_checker: Option<tokio::task::JoinHandle<()>>,
) {
    // Cache invalidation and health sweeps have nothing to persist, so they are
    // cancelled outright. Awaiting the handle is what guarantees the task is
    // really gone and its captured `Arc<AppState>` released.
    for task in [invalidation, link_checker].into_iter().flatten() {
        task.abort();
        let _ = task.await;
    }

    let drained = tokio::time::timeout(std::time::Duration::from_secs(DRAIN_TIMEOUT_SECS), async {
        let _ = analytics.await;
        let _ = webhooks.await;
    })
    .await;
    match drained {
        Ok(()) => tracing::info!("workers drained"),
        Err(_) => tracing::warn!(
            timeout_secs = DRAIN_TIMEOUT_SECS,
            "workers did not drain in time, exiting anyway"
        ),
    }
}

/// Spawns the scheduled Google Sheets sync when it is configured. Opt-in via
/// `QUARK_SHEETS_SYNC_SECS`; lease-coordinated so it is safe on every replica.
fn spawn_sheets_sync(state: &std::sync::Arc<AppState>) {
    // Scheduled Sheets sync (opt-in via QUARK_SHEETS_SYNC_SECS). Lease-coordinated
    // like the link checker so it is safe on every replica; on the single-node
    // LMDB backend the lease is always granted. Never logs the token.
    if let (Some(cfg), Some(api)) = (state.sheets.clone(), state.sheets_api.clone()) {
        // The scheduled sync has no request to read a Host from, so it needs
        // QUARK_PUBLIC_HOST to build correct short URLs. Without it, skip the
        // schedule (on-demand sync still works, using the request Host) rather
        // than write "https://localhost/<code>" links into the sheet.
        if cfg.sync_secs.is_some() && state.public_host.is_none() {
            tracing::info!(
                "sheets sync: scheduled sync disabled (set QUARK_PUBLIC_HOST so short URLs are correct; on-demand sync still works)"
            );
        }
        if let (Some(secs), Some(public_host)) = (cfg.sync_secs, state.public_host.clone()) {
            let store = state.store.clone();
            let sink = state.sink.clone();
            let key = state.key;
            let base_url = format!("https://{public_host}");
            // Per-process holder id for the sync lease (mirrors the health checker).
            let mut hb = [0u8; 8];
            let _ = getrandom::fill(&mut hb);
            let holder: String = format!("sheets_{}", quark::hex(&hb));
            // Lease TTL is a crash-safety cap only: the tick RELEASES the lease
            // when done (see end of loop), so it never stays held between ticks
            // and block the on-demand "Sync now". Capped at 300s (and at the
            // interval) so even a missed release frees it before the next tick.
            let ttl = secs.min(SHEETS_LEASE_TTL_CAP_SECS);
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
                    let tenants = match store.list_tenants().await {
                        Ok(t) => t,
                        Err(e) => {
                            tracing::warn!(error = %e, "sheets sync could not list tenants");
                            continue;
                        }
                    };
                    for t in tenants {
                        let Ok(Some(mut conn)) = store.get_sheets_connection(t.id).await else {
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
                                    sink.as_ref(),
                                    key,
                                    &base_url,
                                    &mut conn,
                                    &token,
                                    quark::now(),
                                    t.id,
                                )
                                .await
                            }
                            Err(e) => Err(e),
                        };
                        if let Err(e) = &outcome {
                            conn.last_status = quark::sheets::SyncStatus::Error(e.to_string());
                            tracing::warn!(error = %e, tenant_id = t.id.0, "sheets sync failed");
                        } else {
                            tracing::info!(tenant_id = t.id.0, "sheets sync completed");
                        }
                        if let Err(e) = store.put_sheets_connection(t.id, &conn).await {
                            tracing::warn!(error = %e, tenant_id = t.id.0, "sheets sync persist failed");
                        }
                    }
                    // Release the lease now that this tick finished so it is not
                    // held between ticks and does not block the on-demand sync.
                    let _ = store.release_sheets_lease(&holder).await;
                }
            });
        }
    }
}

/// Spawns the hourly garbage collection of expired OIDC login sessions.
/// No-op when OIDC is off, since there are no sessions to collect.
fn spawn_session_gc(state: &std::sync::Arc<AppState>) {
    // Garbage-collect expired OIDC login sessions hourly (only when OIDC is on).
    if state.oidc.is_some() {
        let store = state.store.clone();
        tokio::spawn(async move {
            let mut ticker =
                tokio::time::interval(std::time::Duration::from_secs(HOUSEKEEPING_INTERVAL_SECS));
            loop {
                ticker.tick().await;
                if let Err(e) = store.gc_sessions(quark::now()).await {
                    tracing::warn!(error = %e, "session gc failed");
                }
            }
        });
    }
}

/// Spawns the hourly analytics retention purge (LUC-65, GDPR) when a retention
/// window is configured. Unlimited retention spawns nothing.
fn spawn_analytics_purge(state: &std::sync::Arc<AppState>, multi_tenant: bool) {
    // Analytics retention purge (LUC-65, GDPR). Retention window:
    // `QUARK_ANALYTICS_RETENTION_DAYS` (days -> seconds) wins when set and
    // valid; otherwise cloud (`multi_tenant`) defaults to 365 days and
    // OSS/self-host is unlimited. `None` = unlimited => the purge task is NOT
    // spawned. An env value that is SET but fails to parse as a `u64` falls
    // back to the mode default rather than silently disabling retention (see
    // `retention_secs_from`) — only an ABSENT env uses the default directly.
    let retention_env = std::env::var("QUARK_ANALYTICS_RETENTION_DAYS").ok();
    if let Some(v) = &retention_env {
        if v.trim().parse::<u64>().is_err() {
            tracing::warn!(
                value = %v,
                "QUARK_ANALYTICS_RETENTION_DAYS is not a valid non-negative integer, falling \
                 back to the mode default instead of disabling retention"
            );
        }
    }
    let retention_secs: Option<u64> = retention_secs_from(retention_env.as_deref(), multi_tenant);
    if let Some(retention) = retention_secs {
        tracing::info!(retention_secs = retention, "analytics retention configured");
        // Hourly purge task (mirrors the session GC above), fail-open: a purge
        // error is only logged and never blocks serving.
        let store = state.store.clone();
        tokio::spawn(async move {
            let mut ticker =
                tokio::time::interval(std::time::Duration::from_secs(HOUSEKEEPING_INTERVAL_SECS));
            loop {
                ticker.tick().await;
                let cutoff = quark::now().saturating_sub(retention);
                match store.purge_click_events_before(cutoff).await {
                    Ok(n) => {
                        tracing::info!(deleted = n, cutoff_ts = cutoff, "analytics purge completed")
                    }
                    Err(e) => tracing::warn!(error = %e, "analytics purge failed"),
                }
            }
        });
    } else {
        tracing::info!("analytics retention: unlimited (no purge)");
    }
}

#[cfg(test)]
mod retention_tests {
    use super::retention_secs_from;

    #[test]
    fn absent_env_uses_mode_default() {
        assert_eq!(retention_secs_from(None, true), Some(365 * 86_400));
        assert_eq!(retention_secs_from(None, false), None);
    }

    #[test]
    fn valid_env_wins_over_mode_default() {
        assert_eq!(retention_secs_from(Some("30"), true), Some(30 * 86_400));
        assert_eq!(retention_secs_from(Some("30"), false), Some(30 * 86_400));
        // Whitespace is tolerated (matches the previous `.trim()` behavior).
        assert_eq!(retention_secs_from(Some(" 7 "), true), Some(7 * 86_400));
    }

    #[test]
    fn zero_env_means_purge_everything() {
        assert_eq!(retention_secs_from(Some("0"), true), Some(0));
        assert_eq!(retention_secs_from(Some("0"), false), Some(0));
    }

    #[test]
    fn invalid_env_falls_back_to_mode_default_instead_of_unlimited() {
        // This is the LUC-65 review fix: a set-but-invalid value must NOT
        // silently disable retention in cloud (multi_tenant = true).
        assert_eq!(
            retention_secs_from(Some("not-a-number"), true),
            Some(365 * 86_400)
        );
        assert_eq!(retention_secs_from(Some("-5"), true), Some(365 * 86_400));
        assert_eq!(retention_secs_from(Some(""), true), Some(365 * 86_400));
        // In OSS/self-host (multi_tenant = false) the mode default is still
        // unlimited, so invalid input behaves the same as absent there.
        assert_eq!(retention_secs_from(Some("garbage"), false), None);
    }
}
