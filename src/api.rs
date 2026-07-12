use crate::cache::Cache;
use crate::store::{Record, Store};
use crate::{codec, permute};
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct AppState {
    pub cache: Cache,
    pub store: Arc<Store>,
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
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn valida_url(u: &str) -> bool {
    u.starts_with("http://") || u.starts_with("https://")
}

async fn create(
    State(st): State<Arc<AppState>>,
    Json(req): Json<CreateReq>,
) -> Response {
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
    let rec = Record { url: req.url.clone(), expiry, created: now() };

    // aliases: caminho separado; único ponto de checagem de colisão.
    if let Some(alias) = req.alias {
        if codec::from_base62(&alias).is_some() {
            return (StatusCode::BAD_REQUEST, "alias colide com o espaço de código numérico").into_response();
        }
        let id = match st.store.next_id() {
            Ok(id) => id,
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        };
        match st.store.put_alias_and_link(&alias, id, &rec) {
            Ok(true) => {}
            Ok(false) => return (StatusCode::CONFLICT, "alias em uso").into_response(),
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        };
        return Json(CreateResp { code: alias, url: req.url }).into_response();
    }

    // caminho sem alias: id atômico → encode → grava. Sem checagem de colisão.
    let id = match st.store.next_id() {
        Ok(id) => id,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    if id > permute::MAX_ID {
        return (StatusCode::INSUFFICIENT_STORAGE, "espaço de id esgotado").into_response();
    }
    if st.store.put_link(id, &rec).is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let code = codec::to_base62(permute::encode(id, st.key));
    Json(CreateResp { code, url: req.url }).into_response()
}

async fn redirect(
    State(st): State<Arc<AppState>>,
    Path(code): Path<String>,
) -> Response {
    // resolve id: primeiro tenta código numérico; se falhar, tenta alias.
    let id = match codec::from_base62(&code) {
        Some(c) if c <= permute::MAX_ID => permute::decode(c, st.key),
        _ => match st.store.get_alias(&code) {
            Ok(Some(id)) => id,
            Ok(None) => return StatusCode::NOT_FOUND.into_response(),
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        },
    };
    match st.cache.get(id) {
        Ok(Some(rec)) => {
            if let Some(exp) = rec.expiry {
                if now() >= exp {
                    return (StatusCode::GONE, "link expirado").into_response();
                }
            }
            (StatusCode::FOUND, [(header::LOCATION, rec.url)]).into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

async fn health() -> &'static str {
    "ok"
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", post(create))
        .route("/health", get(health))
        .route("/:code", get(redirect))
        .with_state(state)
}
