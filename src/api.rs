use crate::cache::Cache;
use crate::store::{Record, Store};
use crate::{codec, permute};
use axum::extract::{Path, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub struct AppState {
    pub cache: Cache,
    pub store: Arc<dyn Store>,
    pub key: u64,
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

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn valida_url(u: &str) -> bool {
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

async fn create(State(st): State<Arc<AppState>>, Json(req): Json<CreateReq>) -> Response {
    if !valida_url(&req.url) {
        return (StatusCode::BAD_REQUEST, "url inválida").into_response();
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

async fn redirect(State(st): State<Arc<AppState>>, Path(code): Path<String>) -> Response {
    // resolve id: primeiro tenta código numérico; se falhar, tenta alias.
    let id = match codec::from_base62(&code) {
        Some(c) if c <= permute::MAX_ID => permute::decode(c, st.key),
        _ => match st.store.get_alias(&code).await {
            Ok(Some(id)) => id,
            Ok(None) => {
                return (
                    StatusCode::NOT_FOUND,
                    [(header::CACHE_CONTROL, "no-store".to_string())],
                )
                    .into_response()
            }
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        },
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

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", post(create))
        .route("/health", get(health))
        .route("/:code", get(redirect))
        .layer(axum::middleware::from_fn(log_requests))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::{access_log_line, cache_control_for};

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
