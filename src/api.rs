use crate::abuse::{extract_host, is_internal_host};
use crate::analytics::{AnalyticsSink, ClickEvent};
use crate::cache::Cache;
use crate::store::{Record, Store, StoreError};
use crate::{codec, now, permute};
use axum::body::Bytes;
use axum::extract::{ConnectInfo, Path, Query, Request, State};
use axum::http::Method;
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use tower_http::cors::{Any, CorsLayer};

pub struct AppState {
    pub cache: Cache,
    pub store: Arc<dyn Store>,
    pub key: u64,
    pub analytics_tx: tokio::sync::mpsc::Sender<ClickEvent>,
    pub sink: Arc<dyn AnalyticsSink>,
    pub admin_token: Option<String>,
    pub ratelimiter: crate::abuse::ratelimit::RateLimiter,
    pub blocklist: crate::abuse::blocklist::Blocklist,
    pub block_private: bool,
    pub public_host: Option<String>,
    pub real_ip_header: String,
}

#[derive(Deserialize)]
pub struct CreateReq {
    url: String,
    alias: Option<String>,
    ttl: Option<u64>, // segundos a partir de agora
}

#[derive(Serialize)]
pub struct CreateResp {
    code: String,
    url: String,
}

fn is_valid_url(u: &str) -> bool {
    u.starts_with("http://") || u.starts_with("https://")
}

const DEFAULT_MAX_AGE: u64 = 86400;

/// Calcula o valor do header Cache-Control para uma resposta de redirect,
/// respeitando o TTL do link: nunca cacheia além da expiração. Função pura,
/// alvo de TDD.
fn cache_control_for(expiry: Option<u64>, now: u64) -> String {
    match expiry {
        None => format!("public, max-age={}", DEFAULT_MAX_AGE),
        Some(e) if e > now => {
            let max_age = DEFAULT_MAX_AGE.min(e - now);
            format!("public, max-age={}", max_age)
        }
        Some(_) => "no-store".to_string(),
    }
}

/// Exige o admin token para criar — mas só quando um token está configurado.
/// Sem QUARK_ADMIN_TOKEN, criar continua público (shortener aberto).
fn require_admin_for_create(st: &AppState, headers: &HeaderMap) -> Result<(), StatusCode> {
    match st.admin_token.as_deref() {
        None => Ok(()),
        Some(expected) => {
            let provided = headers
                .get("x-admin-token")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if constant_time_eq(provided.as_bytes(), expected.as_bytes()) {
                Ok(())
            } else {
                Err(StatusCode::UNAUTHORIZED)
            }
        }
    }
}

async fn create(
    State(st): State<Arc<AppState>>,
    conn: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    Json(req): Json<CreateReq>,
) -> Response {
    if let Err(status) = require_admin_for_create(&st, &headers) {
        return status.into_response();
    }
    // 1) rate-limit (checagem barata primeiro)
    let ip = client_ip(&headers, &st.real_ip_header, conn.as_ref());
    if !st.ratelimiter.check(&ip, now()).await {
        return (StatusCode::TOO_MANY_REQUESTS, "muitas requisições").into_response();
    }
    // 2) validação de URL (http/https) — já existente
    if !is_valid_url(&req.url) {
        return (StatusCode::BAD_REQUEST, "url inválida").into_response();
    }
    // 3) host do destino (URL sem host é inválida)
    let Some(host) = extract_host(&req.url) else {
        return (StatusCode::BAD_REQUEST, "url sem host").into_response();
    };
    // 4) guarda embutida (rede interna / loop pro próprio host)
    if st.block_private && is_blocked_target(&host, &headers, &st) {
        return (StatusCode::FORBIDDEN, "destino não permitido").into_response();
    }
    // 5) blocklist do banco (domínio + subdomínio)
    if st.blocklist.is_blocked(&host, now()).await {
        return (StatusCode::FORBIDDEN, "destino bloqueado").into_response();
    }

    let expiry = match req.ttl {
        Some(t) => match now().checked_add(t) {
            Some(e) => Some(e),
            None => return (StatusCode::BAD_REQUEST, "ttl inválido").into_response(),
        },
        None => None,
    };
    let rec = Record {
        url: req.url.clone(),
        expiry,
        created: now(),
    };

    // aliases: caminho separado; único ponto de checagem de colisão.
    if let Some(alias) = req.alias {
        if codec::from_base62(&alias).is_some() {
            return (
                StatusCode::BAD_REQUEST,
                "alias colide com o espaço de código numérico",
            )
                .into_response();
        }
        let id = match st.store.next_id().await {
            Ok(id) => id,
            Err(StoreError::IdSpaceExhausted) => {
                return (StatusCode::INSUFFICIENT_STORAGE, "espaço de id esgotado").into_response()
            }
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        };
        match st.store.put_alias_and_link(&alias, id, &rec).await {
            Ok(true) => {}
            Ok(false) => return (StatusCode::CONFLICT, "alias em uso").into_response(),
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        };
        return Json(CreateResp {
            code: alias,
            url: req.url,
        })
        .into_response();
    }

    // caminho sem alias: id atômico → encode → grava. Sem checagem de colisão.
    let id = match st.store.next_id().await {
        Ok(id) => id,
        Err(StoreError::IdSpaceExhausted) => {
            return (StatusCode::INSUFFICIENT_STORAGE, "espaço de id esgotado").into_response()
        }
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    if id > permute::MAX_ID {
        return (StatusCode::INSUFFICIENT_STORAGE, "espaço de id esgotado").into_response();
    }
    if st.store.put_link(id, &rec).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let code = codec::to_base62(permute::encode(id, st.key));
    Json(CreateResp { code, url: req.url }).into_response()
}

/// IP do cliente: header configurável (default CF-Connecting-IP) tem prioridade;
/// senão o IP do socket; senão "unknown" (bucket único, conservador).
fn client_ip(
    headers: &HeaderMap,
    header_name: &str,
    conn: Option<&ConnectInfo<SocketAddr>>,
) -> String {
    if let Some(v) = headers.get(header_name).and_then(|v| v.to_str().ok()) {
        let v = v.trim();
        if !v.is_empty() {
            return v.to_string();
        }
    }
    if let Some(ConnectInfo(addr)) = conn {
        return addr.ip().to_string();
    }
    "unknown".to_string()
}

/// Guarda embutida: destino de rede interna, ou loop pro próprio host do quark.
fn is_blocked_target(host: &str, headers: &HeaderMap, st: &AppState) -> bool {
    if is_internal_host(host) {
        return true;
    }
    // anti-loop: host próprio via QUARK_PUBLIC_HOST ou o header Host da requisição
    let self_host = st.public_host.clone().or_else(|| {
        headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .map(|h| h.split(':').next().unwrap_or(h).to_ascii_lowercase())
    });
    matches!(self_host, Some(sh) if sh == host)
}

/// Resolve um code de URL em id: primeiro tenta código numérico (base62 no
/// domínio); se não for, trata como alias no store. `Ok(Some(id))` resolvido,
/// `Ok(None)` inexistente, `Err` falha de backend. Cada handler mapeia esses
/// casos pra sua própria resposta HTTP (o redirect anexa Cache-Control no 404).
async fn resolve_code(st: &AppState, code: &str) -> Result<Option<u64>, StoreError> {
    match codec::from_base62(code) {
        Some(c) if c <= permute::MAX_ID => Ok(Some(permute::decode(c, st.key))),
        _ => st.store.get_alias(code).await,
    }
}

async fn redirect(
    State(st): State<Arc<AppState>>,
    Path(code): Path<String>,
    headers: HeaderMap,
) -> Response {
    let id = match resolve_code(&st, &code).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                [(header::CACHE_CONTROL, "no-store".to_string())],
            )
                .into_response()
        }
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    match st.cache.get(id).await {
        Ok(Some(rec)) => {
            let now = now();
            if let Some(exp) = rec.expiry {
                if now >= exp {
                    return (
                        StatusCode::GONE,
                        [(header::CACHE_CONTROL, "no-store".to_string())],
                        "link expirado",
                    )
                        .into_response();
                }
            }
            let cache_control = cache_control_for(rec.expiry, now);

            // Captura fire-and-forget: nunca bloqueia nem falha o redirect.
            let ev = ClickEvent {
                id,
                ts: now,
                referer: headers
                    .get(header::REFERER)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string()),
                country: headers
                    .get("cf-ipcountry")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string()),
                user_agent: headers
                    .get(header::USER_AGENT)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string()),
            };
            let _ = st.analytics_tx.try_send(ev);

            (
                StatusCode::FOUND,
                [
                    (header::LOCATION, rec.url),
                    (header::CACHE_CONTROL, cache_control),
                ],
            )
                .into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            [(header::CACHE_CONTROL, "no-store".to_string())],
        )
            .into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

async fn stats(
    State(st): State<Arc<AppState>>,
    Path(code): Path<String>,
    headers: HeaderMap,
) -> Response {
    // endpoint desligado se não há token configurado
    let Some(expected) = st.admin_token.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let provided = headers
        .get("x-admin-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !constant_time_eq(provided.as_bytes(), expected.as_bytes()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let id = match resolve_code(&st, &code).await {
        Ok(Some(id)) => id,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    // link precisa existir; alvo de alias inexistente também é 404 (mesma lógica do redirect)
    match st.store.get_link(id).await {
        Ok(Some(_)) => {}
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
    match st.sink.stats(id).await {
        Ok(Some(s)) => Json(s).into_response(),
        Ok(None) => Json(crate::analytics::Stats {
            aggregates: crate::analytics::Aggregates::default(),
            recent: Vec::new(),
        })
        .into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

#[derive(Deserialize)]
struct BlocklistReq {
    domain: String,
}

/// Autoriza uma requisição admin: `Ok(())` se o token bate; `Err(status)` senão.
/// Sem token configurado → 404 (endpoint desligado); token errado → 401.
/// Retorna `StatusCode` (não `Response`) pra ficar `Copy`/pequeno — evita o lint
/// `result_large_err` do clippy, que dispararia com `Response` no `Err`.
fn admin_guard(st: &AppState, headers: &HeaderMap) -> Result<(), StatusCode> {
    let Some(expected) = st.admin_token.as_deref() else {
        return Err(StatusCode::NOT_FOUND);
    };
    let provided = headers
        .get("x-admin-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !constant_time_eq(provided.as_bytes(), expected.as_bytes()) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(())
}

async fn blocklist_get(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(status) = admin_guard(&st, &headers) {
        return status.into_response();
    }
    match st.store.list_blocked_domains().await {
        Ok(domains) => Json(serde_json::json!({ "domains": domains })).into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

async fn blocklist_add(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(status) = admin_guard(&st, &headers) {
        return status.into_response();
    }
    let req: BlocklistReq = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, "json inválido").into_response(),
    };
    if st.store.add_blocked_domain(&req.domain).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    st.blocklist.invalidate().await;
    StatusCode::OK.into_response()
}

async fn blocklist_delete(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(status) = admin_guard(&st, &headers) {
        return status.into_response();
    }
    let req: BlocklistReq = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, "json inválido").into_response(),
    };
    if st.store.remove_blocked_domain(&req.domain).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    st.blocklist.invalidate().await;
    StatusCode::OK.into_response()
}

#[derive(Deserialize)]
struct ListParams {
    after: Option<u64>,
    limit: Option<usize>,
    q: Option<String>,
}

#[derive(Serialize)]
struct LinkRow {
    id: u64,
    code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    alias: Option<String>,
    url: String,
    expiry: Option<u64>,
    created: u64,
}

async fn admin_links_list(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(p): Query<ListParams>,
) -> Response {
    if let Err(status) = admin_guard(&st, &headers) {
        return status.into_response();
    }
    let limit = p.limit.unwrap_or(50).clamp(1, 500);
    let q = p.q.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let links = match q {
        Some(term) => match st.store.search_links(term, p.after, limit).await {
            Ok(l) => l,
            // Backend sem busca server-side (LMDB): sinaliza ao painel cair
            // pro filtro client-side.
            Err(StoreError::Unsupported) => return StatusCode::NOT_IMPLEMENTED.into_response(),
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        },
        None => match st.store.list_links(p.after, limit).await {
            Ok(l) => l,
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        },
    };
    // mapa id -> alias (um único list_aliases por request)
    let alias_map: std::collections::HashMap<u64, String> = match st.store.list_aliases().await {
        Ok(pairs) => pairs.into_iter().map(|(a, id)| (id, a)).collect(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    // Só há próxima página se veio uma página cheia; página parcial = fim
    // (evita um fetch vazio extra no cliente).
    let next_after = if links.len() == limit {
        links.last().map(|(id, _)| *id)
    } else {
        None
    };
    let rows: Vec<LinkRow> = links
        .into_iter()
        .map(|(id, rec)| LinkRow {
            id,
            code: codec::to_base62(permute::encode(id, st.key)),
            alias: alias_map.get(&id).cloned(),
            url: rec.url,
            expiry: rec.expiry,
            created: rec.created,
        })
        .collect();
    Json(serde_json::json!({ "links": rows, "next_after": next_after })).into_response()
}

/// Resolve o code em (id, alias_opcional). Se o code é numérico, não há alias
/// a remover; se é uma string de alias, devolve o alias pra apagar junto.
async fn resolve_for_admin(
    st: &AppState,
    code: &str,
) -> Result<Option<(u64, Option<String>)>, StoreError> {
    match codec::from_base62(code) {
        Some(c) if c <= permute::MAX_ID => Ok(Some((permute::decode(c, st.key), None))),
        _ => match st.store.get_alias(code).await? {
            Some(id) => Ok(Some((id, Some(code.to_string())))),
            None => Ok(None),
        },
    }
}

async fn admin_link_delete(
    State(st): State<Arc<AppState>>,
    Path(code): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(status) = admin_guard(&st, &headers) {
        return status.into_response();
    }
    let (id, alias) = match resolve_for_admin(&st, &code).await {
        Ok(Some(v)) => v,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    // 404 se o link em si não existe
    match st.store.get_link(id).await {
        Ok(Some(_)) => {}
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
    if st.store.delete_link(id).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if let Some(a) = alias {
        let _ = st.store.delete_alias(&a).await;
    }
    st.cache.invalidate(id).await;
    StatusCode::OK.into_response()
}

async fn admin_link_patch(
    State(st): State<Arc<AppState>>,
    Path(code): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(status) = admin_guard(&st, &headers) {
        return status.into_response();
    }
    let (id, _) = match resolve_for_admin(&st, &code).await {
        Ok(Some(v)) => v,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let mut rec = match st.store.get_link(id).await {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    // corpo: chaves ausentes = mantém; "url" atualiza; "ttl": número recomputa
    // expiry (now+ttl), null remove a expiração.
    let patch: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return (StatusCode::BAD_REQUEST, "json inválido").into_response(),
    };
    if let Some(u) = patch.get("url") {
        let s = match u.as_str() {
            Some(s) if is_valid_url(s) => s,
            _ => return (StatusCode::BAD_REQUEST, "url inválida").into_response(),
        };
        let Some(host) = extract_host(s) else {
            return (StatusCode::BAD_REQUEST, "url sem host").into_response();
        };
        if st.block_private && is_blocked_target(&host, &headers, &st) {
            return (StatusCode::FORBIDDEN, "destino não permitido").into_response();
        }
        if st.blocklist.is_blocked(&host, now()).await {
            return (StatusCode::FORBIDDEN, "destino bloqueado").into_response();
        }
        rec.url = s.to_string();
    }
    if let Some(ttl) = patch.get("ttl") {
        if ttl.is_null() {
            rec.expiry = None;
        } else if let Some(secs) = ttl.as_u64() {
            match now().checked_add(secs) {
                Some(e) => rec.expiry = Some(e),
                None => return (StatusCode::BAD_REQUEST, "ttl inválido").into_response(),
            }
        } else {
            return (StatusCode::BAD_REQUEST, "ttl inválido").into_response();
        }
    }
    if st.store.put_link(id, &rec).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    st.cache.invalidate(id).await;
    StatusCode::OK.into_response()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

async fn health() -> &'static str {
    "ok"
}

/// Formata uma linha de log de acesso em JSON. Função pura: sem I/O, fácil de testar.
fn access_log_line(method: &str, path: &str, status: u16, latency_ms: f64) -> String {
    let latency_ms = (latency_ms * 1000.0).round() / 1000.0;
    serde_json::json!({
        "method": method,
        "path": path,
        "status": status,
        "latency_ms": latency_ms,
    })
    .to_string()
}

/// Middleware que loga uma linha JSON por request no stdout (Coolify captura stdout).
/// Puramente observacional: não altera a resposta.
async fn log_requests(req: Request, next: Next) -> Response {
    let start = Instant::now();
    let method = req.method().to_string();
    let path = req.uri().path().to_string();

    let response = next.run(req).await;

    let latency_ms = start.elapsed().as_secs_f64() * 1000.0;
    let status = response.status().as_u16();
    println!("{}", access_log_line(&method, &path, status, latency_ms));

    response
}

/// Origens de CORS a partir da env `QUARK_CORS_ORIGINS` (lista por vírgula).
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
        .route("/:code", get(redirect))
        .route("/:code/stats", get(stats))
        .route(
            "/admin/blocklist",
            get(blocklist_get)
                .post(blocklist_add)
                .delete(blocklist_delete),
        )
        .route("/admin/links", get(admin_links_list))
        .route(
            "/admin/links/:code",
            axum::routing::delete(admin_link_delete).patch(admin_link_patch),
        )
        .with_state(state);

    // CORS é opt-in via QUARK_CORS_ORIGINS: sem origens configuradas, nenhum
    // header de CORS é adicionado (comportamento atual preservado).
    let app = if origins.is_empty() {
        app
    } else {
        let list: Vec<axum::http::HeaderValue> =
            origins.iter().filter_map(|o| o.parse().ok()).collect();
        let cors = CorsLayer::new()
            .allow_origin(list)
            .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::DELETE])
            .allow_headers(Any);
        app.layer(cors)
    };

    // Log de acesso por request é opt-in: em alta vazão, o println! síncrono
    // por request serializa tudo na lock do stdout (I/O do Docker json-file).
    // Fica desligado por padrão; ativa com QUARK_ACCESS_LOG=1.
    if std::env::var("QUARK_ACCESS_LOG").is_ok() {
        app.layer(axum::middleware::from_fn(log_requests))
    } else {
        app
    }
}

#[cfg(test)]
mod tests {
    use super::{access_log_line, cache_control_for, parse_cors_origins};

    #[test]
    fn parse_cors_origins_split_e_trim() {
        assert_eq!(parse_cors_origins(None), Vec::<String>::new());
        assert_eq!(parse_cors_origins(Some("".into())), Vec::<String>::new());
        assert_eq!(
            parse_cors_origins(Some(" https://a.com , https://b.com ".into())),
            vec!["https://a.com".to_string(), "https://b.com".to_string()]
        );
    }

    #[test]
    fn cache_control_sem_expiry_usa_default() {
        assert_eq!(cache_control_for(None, 1_000), "public, max-age=86400");
    }

    #[test]
    fn cache_control_com_expiry_futuro_usa_diferenca() {
        let now = 1_000;
        assert_eq!(
            cache_control_for(Some(now + 100), now),
            "public, max-age=100"
        );
    }

    #[test]
    fn cache_control_com_expiry_futuro_distante_capa_em_default() {
        let now = 1_000;
        assert_eq!(
            cache_control_for(Some(now + 999_999), now),
            "public, max-age=86400"
        );
    }

    #[test]
    fn cache_control_com_expiry_passado_no_store() {
        let now = 1_000;
        assert_eq!(cache_control_for(Some(now - 1), now), "no-store");
    }

    #[test]
    fn access_log_line_is_valid_json_with_expected_fields() {
        let line = access_log_line("GET", "/abc", 302, 0.4139);
        let v: serde_json::Value =
            serde_json::from_str(&line).expect("access_log_line deve produzir JSON válido");
        assert_eq!(v["method"], "GET");
        assert_eq!(v["path"], "/abc");
        assert_eq!(v["status"], 302);
        assert_eq!(v["latency_ms"], 0.414);
    }

    #[test]
    fn access_log_line_escapes_special_characters_in_path() {
        let path = "/a\"b\\c";
        let line = access_log_line("GET", path, 200, 1.0);
        let v: serde_json::Value = serde_json::from_str(&line)
            .expect("access_log_line deve escapar corretamente e continuar sendo JSON válido");
        assert_eq!(v["path"], path);
    }
}
