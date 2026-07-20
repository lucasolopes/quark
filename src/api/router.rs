use super::*;

pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

pub(crate) async fn health() -> &'static str {
    "ok"
}

/// Formats an access log line as JSON. Pure function: no I/O, easy to test.
pub(crate) fn access_log_line(method: &str, path: &str, status: u16, latency_ms: f64) -> String {
    let latency_ms = (latency_ms * 1000.0).round() / 1000.0;
    serde_json::json!({
        "method": method,
        "path": path,
        "status": status,
        "latency_ms": latency_ms,
    })
    .to_string()
}

/// Middleware that logs one JSON line per request to stdout (Coolify captures stdout).
/// Purely observational: doesn't alter the response.
pub(crate) async fn log_requests(req: Request, next: Next) -> Response {
    let start = Instant::now();
    let method = req.method().to_string();
    let path = req.uri().path().to_string();

    let response = next.run(req).await;

    let latency_ms = start.elapsed().as_secs_f64() * 1000.0;
    let status = response.status().as_u16();
    println!("{}", access_log_line(&method, &path, status, latency_ms));

    response
}

/// CORS origins from the `QUARK_CORS_ORIGINS` env var (comma-separated list).
pub fn parse_cors_origins(raw: Option<String>) -> Vec<String> {
    match raw {
        None => Vec::new(),
        Some(s) => s
            .split(',')
            .map(|o| o.trim().to_string())
            .filter(|o| !o.is_empty())
            .collect(),
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    let origins = parse_cors_origins(std::env::var("QUARK_CORS_ORIGINS").ok());
    router_with_cors(state, origins)
}

pub fn router_with_cors(state: Arc<AppState>, origins: Vec<String>) -> Router {
    let app = Router::new()
        .route("/", post(create))
        .route("/health", get(health))
        .route("/:code", get(redirect).post(unlock))
        .route("/:code/stats", get(stats))
        .route("/admin/stats", get(admin_stats))
        .route("/admin/links", get(admin_links_list))
        .route("/admin/import", post(admin_import))
        // Static `bulk` and the `:code` param route coexist: matchit (axum's
        // router) gives the static segment priority, so `POST /admin/links/bulk`
        // hits this handler and never matches `:code` (which only serves
        // DELETE/PATCH anyway). Verified by `admin_bulk_add_tag_*` tests.
        .route("/admin/links/bulk", post(admin_links_bulk))
        .route(
            "/admin/links/:code",
            axum::routing::delete(admin_link_delete).patch(admin_link_patch),
        )
        .route(
            "/admin/links/:code/alert",
            axum::routing::get(admin_link_alert_get)
                .put(admin_link_alert_put)
                .delete(admin_link_alert_delete),
        )
        .route(
            "/admin/links/:code/analytics",
            axum::routing::delete(admin_link_analytics_delete),
        )
        .route(
            "/admin/webhooks",
            get(admin_webhooks_list).post(admin_webhooks_create),
        )
        .route(
            "/admin/webhooks/:id",
            axum::routing::patch(admin_webhooks_patch).delete(admin_webhooks_delete),
        )
        .route("/admin/webhooks/:id/test", post(admin_webhooks_test))
        .route("/admin/login", get(oidc_login))
        .route("/admin/callback", get(oidc_callback))
        .route("/admin/logout", post(oidc_logout))
        .route("/admin/me", get(admin_me))
        .route("/admin/tenants", post(admin_tenants_create))
        .route("/admin/workspace/switch", post(admin_workspace_switch))
        .route(
            "/admin/oidc-config",
            get(admin_oidc_config_get)
                .put(admin_oidc_config_put)
                .delete(admin_oidc_config_delete),
        )
        .route(
            "/admin/domains",
            get(admin_domains_list).post(admin_domains_create),
        )
        .route(
            "/admin/domains/:id",
            axum::routing::delete(admin_domains_delete),
        )
        .route("/admin/domains/:id/verify", post(admin_domains_verify))
        .route(
            "/admin/sso-domains",
            get(admin_sso_domains_list).post(admin_sso_domains_create),
        )
        .route(
            "/admin/sso-domains/:id",
            axum::routing::delete(admin_sso_domains_delete),
        )
        .route(
            "/admin/sso-domains/:id/verify",
            post(admin_sso_domains_verify),
        )
        .route("/admin/sso/discover", get(sso_discover))
        .route(
            "/admin/invites",
            get(admin_invites_list).post(admin_invites_create),
        )
        .route(
            "/admin/invites/:id",
            axum::routing::delete(admin_invites_delete),
        )
        .route("/admin/invites/:token/accept", post(admin_invites_accept))
        .route("/admin/integrations/sheets/connect", get(sheets_connect))
        .route("/admin/integrations/sheets/callback", get(sheets_callback))
        .route("/admin/integrations/sheets/sync", post(sheets_sync))
        .route("/admin/integrations/sheets/status", get(sheets_status))
        .route(
            "/admin/integrations/sheets",
            axum::routing::delete(sheets_disconnect),
        )
        .route("/admin/tags", get(admin_tags_list))
        .route("/admin/folders", get(admin_folders_list))
        .route(
            "/admin/tokens",
            get(admin_tokens_list).post(admin_tokens_create),
        )
        .route(
            "/admin/tokens/:id",
            axum::routing::delete(admin_tokens_delete),
        )
        .route(
            "/admin/pixels",
            get(admin_pixels_list).post(admin_pixels_create),
        )
        .route(
            "/admin/pixels/:id",
            axum::routing::delete(admin_pixels_delete),
        )
        .route(
            "/.well-known/apple-app-site-association",
            get(wellknown_aasa),
        )
        .route("/apple-app-site-association", get(wellknown_aasa))
        .route("/.well-known/assetlinks.json", get(wellknown_assetlinks))
        .route(
            "/admin/wellknown/:name",
            get(admin_wellknown_get)
                .put(admin_wellknown_put)
                .delete(admin_wellknown_delete),
        )
        .with_state(state);

    let app = if origins.is_empty() {
        app
    } else {
        let list: Vec<axum::http::HeaderValue> =
            origins.iter().filter_map(|o| o.parse().ok()).collect();
        let cors = CorsLayer::new()
            .allow_origin(list)
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::PATCH,
                Method::DELETE,
            ])
            // Specific headers (not `Any`) because credentials are allowed, and
            // `*` is invalid with credentials. Allow credentials so the OIDC
            // session cookie is accepted on a cross-origin panel.
            .allow_headers([
                header::CONTENT_TYPE,
                axum::http::HeaderName::from_static("x-admin-token"),
                axum::http::HeaderName::from_static("x-quark-csrf"),
            ])
            .allow_credentials(true);
        app.layer(cors)
    };

    if std::env::var("QUARK_ACCESS_LOG").is_ok() {
        app.layer(axum::middleware::from_fn(log_requests))
    } else {
        app
    }
}
