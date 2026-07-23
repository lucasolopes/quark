use super::*;

#[derive(Deserialize)]
pub(crate) struct ListParams {
    after: Option<u64>,
    limit: Option<usize>,
    q: Option<String>,
    tag: Option<String>,
    folder: Option<String>,
    /// `broken` restricts the list to links whose last health probe failed.
    health: Option<String>,
    /// `active` restricts the list to active links (unexpired and under their
    /// visit cap); any other value (or absent) lists everything.
    status: Option<String>,
}

/// Health of a link's destination as exposed to the panel (never includes
/// anything sensitive; omitted from a row when the link was never probed).
#[derive(Serialize)]
pub(crate) struct HealthInfo {
    healthy: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<u16>,
    checked_at: u64,
}

#[derive(Serialize)]
pub(crate) struct LinkRow {
    id: u64,
    code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    alias: Option<String>,
    url: String,
    expiry: Option<u64>,
    created: u64,
    tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_visits: Option<u32>,
    visits: u64,
    rules: Vec<Rule>,
    variants: Vec<Variant>,
    #[serde(skip_serializing_if = "Option::is_none")]
    app_ios: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    app_android: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    folder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fallback_url: Option<String>,
    /// Whether the link is password-protected. The hash itself is never exposed.
    has_password: bool,
    /// Destination health from the background checker; omitted when unchecked.
    #[serde(skip_serializing_if = "Option::is_none")]
    health: Option<HealthInfo>,
}

pub(crate) async fn admin_links_list(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(p): Query<ListParams>,
) -> Response {
    let prin = match admin_guard(&st, &headers, Scope::LinksRead).await {
        Ok(prin) => prin,
        Err(status) => return status.into_response(),
    };
    let limit = p
        .limit
        .unwrap_or(DEFAULT_PAGE_LIMIT)
        .clamp(1, MAX_PAGE_LIMIT);
    let q = p.q.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let tag = p
        .tag
        .as_deref()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty());
    let tag = tag.as_deref();
    let folder = p.folder.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let broken_only = p.health.as_deref() == Some("broken");
    let active_only = p.status.as_deref() == Some("active");
    // The `broken` filter is driven by the health table (a small set),
    // cursor-paginated by id, so each page carries real broken rows (search `q`
    // is ignored for this filter; tag/folder still apply). Otherwise the normal
    // link listing/search runs.
    let (links, next_after): (Vec<(u64, Record)>, Option<u64>) = if broken_only {
        let ids = match st.store.list_broken_link_ids(prin.tenant).await {
            Ok(v) => v,
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        };
        let mut picked: Vec<(u64, Record)> = Vec::new();
        let mut last: Option<u64> = None;
        for id in ids.into_iter().filter(|&id| p.after.is_none_or(|a| id > a)) {
            let rec = match st.store.get_link(prin.tenant, id).await {
                Ok(Some(r)) => r,
                Ok(None) => continue,
                Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
            };
            if let Some(t) = tag {
                if !rec.tags.iter().any(|x| x == t) {
                    continue;
                }
            }
            if let Some(f) = folder {
                if !rec
                    .folder
                    .as_deref()
                    .is_some_and(|x| x.eq_ignore_ascii_case(f))
                {
                    continue;
                }
            }
            last = Some(id);
            picked.push((id, rec));
            if picked.len() == limit {
                break;
            }
        }
        let next = if picked.len() == limit { last } else { None };
        (picked, next)
    } else {
        let links = match q {
            Some(term) => match st
                .store
                .search_links(prin.tenant, term, p.after, limit, tag, folder, active_only)
                .await
            {
                Ok(l) => l,
                Err(StoreError::Unsupported) => return StatusCode::NOT_IMPLEMENTED.into_response(),
                Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
            },
            None => match st
                .store
                .list_links(prin.tenant, p.after, limit, tag, folder, active_only)
                .await
            {
                Ok(l) => l,
                Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
            },
        };
        let next = if links.len() == limit {
            links.last().map(|(id, _)| *id)
        } else {
            None
        };
        (links, next)
    };
    let alias_map: std::collections::HashMap<u64, String> =
        match st.store.list_aliases(prin.tenant).await {
            Ok(pairs) => pairs.into_iter().map(|(a, id)| (id, a)).collect(),
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        };
    // Fetch health for just this page's ids (not the whole table).
    let page_ids: Vec<u64> = links.iter().map(|(id, _)| *id).collect();
    let health_map: std::collections::HashMap<u64, LinkHealth> =
        match st.store.link_health_for(prin.tenant, &page_ids).await {
            Ok(v) => v.into_iter().collect(),
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        };
    // Real click totals for this page's ids in one round trip (LUC-89): the
    // "Visitas" column shows actual clicks (analytics), matching the Analytics
    // view, not the `max_visits` enforcement counter (0 for links with no
    // limit). `max_visits` stays as the denominator when a limit is set.
    let visits_map: std::collections::HashMap<u64, u64> =
        match st.sink.click_totals(&page_ids).await {
            Ok(v) => v,
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        };
    let mut rows: Vec<LinkRow> = Vec::with_capacity(links.len());
    for (id, rec) in links {
        let health = health_map.get(&id);
        let visits = visits_map.get(&id).copied().unwrap_or(0);
        rows.push(LinkRow {
            id,
            code: st.encode_code(id),
            alias: alias_map.get(&id).cloned(),
            url: rec.url,
            expiry: rec.expiry,
            created: rec.created,
            tags: rec.tags,
            max_visits: rec.max_visits,
            visits,
            rules: rec.rules,
            variants: rec.variants,
            app_ios: rec.app_ios,
            app_android: rec.app_android,
            folder: rec.folder,
            fallback_url: rec.fallback_url,
            has_password: rec.password_hash.is_some(),
            health: health.map(|h| HealthInfo {
                healthy: h.healthy,
                status: h.status,
                checked_at: h.checked_at,
            }),
        });
    }
    Json(serde_json::json!({ "links": rows, "next_after": next_after })).into_response()
}

/// `GET /admin/tags`: the distinct set of tags across all links, for the
/// panel's filter control.
pub(crate) async fn admin_tags_list(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let p = match admin_guard(&st, &headers, Scope::LinksRead).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    match st.store.list_tags(p.tenant).await {
        Ok(tags) => {
            let rows: Vec<serde_json::Value> = tags
                .into_iter()
                .map(|(name, count)| serde_json::json!({ "name": name, "count": count }))
                .collect();
            Json(serde_json::json!({ "tags": rows })).into_response()
        }
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// `GET /admin/folders`: the distinct folder names with their link counts, for
/// the panel's folder selector and filter control.
pub(crate) async fn admin_folders_list(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let p = match admin_guard(&st, &headers, Scope::LinksRead).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    match st.store.list_folders(p.tenant).await {
        Ok(folders) => {
            let rows: Vec<serde_json::Value> = folders
                .into_iter()
                .map(|(name, count)| serde_json::json!({ "name": name, "count": count }))
                .collect();
            Json(serde_json::json!({ "folders": rows })).into_response()
        }
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// Resolves the code into (id, optional_alias). If the code is numeric, there's no
/// alias to remove; if it's an alias string, returns the alias to delete alongside it.
///
/// Alias lookup is scoped by the caller's tenant default domain (subdomain on
/// cloud, `SHARED_DOMAIN_ID` on OSS/default tenant) — the same namespace
/// `create` stamps the alias into. See `default_domain_id`, `resolve_code`.
pub(crate) async fn resolve_for_admin(
    st: &AppState,
    tenant: crate::tenant::TenantId,
    code: &str,
) -> Result<Option<(u64, Option<String>)>, StoreError> {
    match codec::from_base62(code) {
        Some(c) if c <= permute::MAX_ID => Ok(Some((permute::decode(c, st.key), None))),
        _ => {
            let domain_id = default_domain_id(st, tenant).await;
            match st.store.get_alias(domain_id, code).await? {
                Some(id) => Ok(Some((id, Some(code.to_string())))),
                None => Ok(None),
            }
        }
    }
}

pub(crate) async fn admin_link_delete(
    State(st): State<Arc<AppState>>,
    Path(code): Path<String>,
    headers: HeaderMap,
) -> Response {
    let p = match admin_guard(&st, &headers, Scope::LinksWrite).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    let (id, alias) = match resolve_for_admin(&st, p.tenant, &code).await {
        Ok(Some(v)) => v,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let rec = match st.store.get_link(p.tenant, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let canonical_code = st.encode_code(id);
    let ev = WebhookEvent {
        event_type: EventType::LinkDeleted,
        body: webhook_event_payload(
            EventType::LinkDeleted,
            &canonical_code,
            &rec.url,
            alias.as_deref(),
            rec.expiry,
            rec.created,
            None,
        ),
        tenant_id: p.tenant,
    };
    let rows = st.webhooks.lifecycle_deliveries(p.tenant, &ev).await;
    if st.store.delete_link_tx(p.tenant, id, &rows).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if let Some(a) = &alias {
        let _ = st.store.delete_alias(p.tenant, a).await;
    }
    st.cache.invalidate(id).await;
    st.webhooks.emit_if_in_memory(ev);
    // 204 to match the other admin deletes (LUC-51): a successful delete has no body.
    StatusCode::NO_CONTENT.into_response()
}

/// `DELETE /admin/links/:code/analytics` — GDPR right-to-erasure (LUC-65):
/// erases the link's analytics (click events + counters + stats), scoped to
/// the caller's tenant, WITHOUT deleting the link itself. Returns 204. A
/// link with no analytics is not an error (the erasure is idempotent).
pub(crate) async fn admin_link_analytics_delete(
    State(st): State<Arc<AppState>>,
    Path(code): Path<String>,
    headers: HeaderMap,
) -> Response {
    let p = match admin_guard(&st, &headers, Scope::LinksWrite).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    let (id, _alias) = match resolve_for_admin(&st, p.tenant, &code).await {
        Ok(Some(v)) => v,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    // Confirm the link exists and belongs to this tenant before erasing its
    // analytics (a syntactically valid code always decodes to an id, so this
    // is what enforces existence + tenant scope, mirroring `admin_link_delete`).
    match st.store.get_link(p.tenant, id).await {
        Ok(Some(_)) => {}
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
    if st.store.delete_link_analytics(p.tenant, id).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    StatusCode::NO_CONTENT.into_response()
}

/// Body of `PUT /admin/links/:code/alert`: the click-threshold alert rule
/// (LUC-38). `threshold` clicks within `window_secs` seconds fire
/// `link.threshold_reached` once per window.
#[derive(Deserialize)]
pub(crate) struct AlertReq {
    threshold: u32,
    window_secs: u64,
}

/// Minimum accepted alert window (seconds), a floor coherent with the other
/// timers in the system.
pub(crate) const MIN_ALERT_WINDOW_SECS: u64 = 60;

/// `GET /admin/links/:code/alert` — the link's current click-threshold alert
/// rule, or `null` when unset (LUC-66). Added so the panel can show the
/// existing rule before editing it, without piggybacking on the PUT response
/// (which only exists after a save).
pub(crate) async fn admin_link_alert_get(
    State(st): State<Arc<AppState>>,
    Path(code): Path<String>,
    headers: HeaderMap,
) -> Response {
    let p = match admin_guard(&st, &headers, Scope::LinksRead).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    let (id, _alias) = match resolve_for_admin(&st, p.tenant, &code).await {
        Ok(Some(v)) => v,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    match st.store.get_alert_rule(p.tenant, id).await {
        Ok(rule) => Json(serde_json::json!(rule)).into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// `PUT /admin/links/:code/alert` — sets (or replaces) the link's
/// click-threshold alert rule. Validates `threshold >= 1` and
/// `window_secs >= 60`. Returns 200 on success.
pub(crate) async fn admin_link_alert_put(
    State(st): State<Arc<AppState>>,
    Path(code): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let p = match admin_guard(&st, &headers, Scope::LinksWrite).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    let req: AlertReq = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid body").into_response(),
    };
    if req.threshold < 1 {
        return (StatusCode::BAD_REQUEST, "threshold must be >= 1").into_response();
    }
    if req.window_secs < MIN_ALERT_WINDOW_SECS {
        return (StatusCode::BAD_REQUEST, "window_secs must be >= 60").into_response();
    }
    let (id, _alias) = match resolve_for_admin(&st, p.tenant, &code).await {
        Ok(Some(v)) => v,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    // Confirm the link exists (and belongs to this tenant) before writing a rule.
    match st.store.get_link(p.tenant, id).await {
        Ok(Some(_)) => {}
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
    let rule = AlertRule {
        threshold: req.threshold,
        window_secs: req.window_secs,
    };
    if st.store.put_alert_rule(p.tenant, id, &rule).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    (StatusCode::OK, axum::Json(serde_json::json!(rule))).into_response()
}

/// `DELETE /admin/links/:code/alert` — removes the link's alert rule. Returns
/// 204; a missing rule is not an error.
pub(crate) async fn admin_link_alert_delete(
    State(st): State<Arc<AppState>>,
    Path(code): Path<String>,
    headers: HeaderMap,
) -> Response {
    let p = match admin_guard(&st, &headers, Scope::LinksWrite).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    let (id, _alias) = match resolve_for_admin(&st, p.tenant, &code).await {
        Ok(Some(v)) => v,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    if st.store.delete_alert_rule(p.tenant, id).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    StatusCode::NO_CONTENT.into_response()
}

pub(crate) async fn admin_link_patch(
    State(st): State<Arc<AppState>>,
    Path(code): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let p = match admin_guard(&st, &headers, Scope::LinksWrite).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    let (id, alias) = match resolve_for_admin(&st, p.tenant, &code).await {
        Ok(Some(v)) => v,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let mut rec = match st.store.get_link(p.tenant, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let patch: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid json").into_response(),
    };
    if let Some(u) = patch.get("url") {
        let s = match u.as_str() {
            Some(s) if is_valid_url(s) => s,
            _ => return (StatusCode::BAD_REQUEST, "invalid url").into_response(),
        };
        let Some(host) = extract_host(s) else {
            return (StatusCode::BAD_REQUEST, "url without host").into_response();
        };
        if st.block_private && is_blocked_target(&host, &headers, &st).await {
            return (StatusCode::FORBIDDEN, "destination not allowed").into_response();
        }
        rec.url = s.to_string();
    }
    if let Some(ttl) = patch.get("ttl") {
        if ttl.is_null() {
            rec.expiry = None;
        } else if let Some(secs) = ttl.as_u64() {
            match now().checked_add(secs) {
                Some(e) => rec.expiry = Some(e),
                None => return (StatusCode::BAD_REQUEST, "invalid ttl").into_response(),
            }
        } else {
            return (StatusCode::BAD_REQUEST, "invalid ttl").into_response();
        }
    }
    if let Some(t) = patch.get("tags") {
        match t {
            serde_json::Value::Array(items) => {
                let mut raw = Vec::with_capacity(items.len());
                for item in items {
                    match item.as_str() {
                        Some(s) => raw.push(s.to_string()),
                        None => return (StatusCode::BAD_REQUEST, "invalid tags").into_response(),
                    }
                }
                rec.tags = normalize_tags(raw);
            }
            _ => return (StatusCode::BAD_REQUEST, "invalid tags").into_response(),
        }
    }
    if let Some(mv) = patch.get("max_visits") {
        if mv.is_null() {
            rec.max_visits = None;
        } else if let Some(n) = mv.as_u64().and_then(|n| u32::try_from(n).ok()) {
            rec.max_visits = normalize_max_visits(Some(n));
        } else {
            return (StatusCode::BAD_REQUEST, "invalid max_visits").into_response();
        }
    }
    if let Some(r) = patch.get("rules") {
        let parsed: Vec<Rule> = match serde_json::from_value(r.clone()) {
            Ok(v) => v,
            Err(_) => return (StatusCode::BAD_REQUEST, "invalid rules").into_response(),
        };
        match validate_rules(parsed, &headers, &st).await {
            Ok(v) => rec.rules = v,
            Err(resp) => return resp,
        }
    }
    if let Some(v) = patch.get("variants") {
        let variants: Vec<Variant> = match serde_json::from_value(v.clone()) {
            Ok(vs) => vs,
            Err(_) => return (StatusCode::BAD_REQUEST, "invalid variants").into_response(),
        };
        if let Err(resp) = validate_variants(&variants, &headers, &st).await {
            return resp;
        }
        rec.variants = variants;
    }
    if let Some(v) = patch.get("app_ios") {
        if v.is_null() {
            rec.app_ios = None;
        } else if let Some(s) = v.as_str() {
            if let Err(status) = app_destination_ok(&st, &headers, s).await {
                return (status, "invalid app destination").into_response();
            }
            rec.app_ios = Some(s.to_string());
        } else {
            return (StatusCode::BAD_REQUEST, "invalid app destination").into_response();
        }
    }
    if let Some(v) = patch.get("app_android") {
        if v.is_null() {
            rec.app_android = None;
        } else if let Some(s) = v.as_str() {
            if let Err(status) = app_destination_ok(&st, &headers, s).await {
                return (status, "invalid app destination").into_response();
            }
            rec.app_android = Some(s.to_string());
        } else {
            return (StatusCode::BAD_REQUEST, "invalid app destination").into_response();
        }
    }
    if let Some(v) = patch.get("folder") {
        if v.is_null() {
            rec.folder = None;
        } else if let Some(s) = v.as_str() {
            rec.folder = normalize_folder(Some(s.to_string()));
        } else {
            return (StatusCode::BAD_REQUEST, "invalid folder").into_response();
        }
    }
    if let Some(v) = patch.get("fallback_url") {
        if v.is_null() {
            rec.fallback_url = None;
        } else if let Some(s) = v.as_str() {
            let s = s.trim();
            if s.is_empty() {
                rec.fallback_url = None;
            } else if let Err(status) = app_destination_ok(&st, &headers, s).await {
                return (status, "invalid fallback url").into_response();
            } else {
                rec.fallback_url = Some(s.to_string());
            }
        } else {
            return (StatusCode::BAD_REQUEST, "invalid fallback url").into_response();
        }
    }
    if let Some(v) = patch.get("password") {
        if v.is_null() {
            rec.password_hash = None;
        } else if let Some(s) = v.as_str() {
            let s = s.trim();
            if s.is_empty() {
                rec.password_hash = None;
            } else {
                let pw = s.to_string();
                match tokio::task::spawn_blocking(move || crate::password::hash_password(&pw)).await
                {
                    Ok(Ok(h)) => rec.password_hash = Some(h),
                    _ => {
                        return (StatusCode::INTERNAL_SERVER_ERROR, "could not hash password")
                            .into_response()
                    }
                }
            }
        } else {
            return (StatusCode::BAD_REQUEST, "invalid password").into_response();
        }
    }
    let canonical_code = st.encode_code(id);
    let ev = WebhookEvent {
        event_type: EventType::LinkUpdated,
        body: webhook_event_payload(
            EventType::LinkUpdated,
            &canonical_code,
            &rec.url,
            alias.as_deref(),
            rec.expiry,
            rec.created,
            None,
        ),
        tenant_id: p.tenant,
    };
    let rows = st.webhooks.lifecycle_deliveries(p.tenant, &ev).await;
    if st
        .store
        .put_link_tx(p.tenant, id, &rec, &rows)
        .await
        .is_err()
    {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    st.cache.invalidate(id).await;
    st.webhooks.emit_if_in_memory(ev);
    StatusCode::OK.into_response()
}

/// Max codes accepted in a single `POST /admin/links/bulk` request. Aligned
/// with the list page size so a "select all on this page" never overflows it;
/// beyond this the request is rejected with 400 rather than processed.
pub(crate) const MAX_BULK: usize = 500;

/// The bulk operation to apply to each selected link.
#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BulkOp {
    Delete,
    AddTag,
    RemoveTag,
    SetFolder,
}

#[derive(Deserialize)]
pub(crate) struct BulkReq {
    codes: Vec<String>,
    op: BulkOp,
    #[serde(default)]
    value: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct BulkItemResult {
    code: String,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct BulkResp {
    ok: usize,
    failed: usize,
    results: Vec<BulkItemResult>,
}

/// Applies one bulk operation to a single link, reusing the exact per-link
/// mutation path of `admin_link_delete` / `admin_link_patch`: resolve → get →
/// mutate `rec` → lifecycle `WebhookEvent` → `put_link_tx`/`delete_link_tx` →
/// cache invalidate → `emit_if_in_memory`. Errors are returned per item so the
/// caller can keep going with the rest of the batch.
pub(crate) async fn bulk_apply_one(
    st: &AppState,
    tenant: crate::tenant::TenantId,
    code: &str,
    op: BulkOp,
    value: Option<&str>,
) -> Result<(), String> {
    let (id, alias) = match resolve_for_admin(st, tenant, code).await {
        Ok(Some(v)) => v,
        Ok(None) => return Err("not found".to_string()),
        Err(_) => return Err("store error".to_string()),
    };
    let mut rec = match st.store.get_link(tenant, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return Err("not found".to_string()),
        Err(_) => return Err("store error".to_string()),
    };
    let canonical_code = st.encode_code(id);

    if op == BulkOp::Delete {
        let ev = WebhookEvent {
            event_type: EventType::LinkDeleted,
            body: webhook_event_payload(
                EventType::LinkDeleted,
                &canonical_code,
                &rec.url,
                alias.as_deref(),
                rec.expiry,
                rec.created,
                None,
            ),
            tenant_id: tenant,
        };
        let rows = st.webhooks.lifecycle_deliveries(tenant, &ev).await;
        if st.store.delete_link_tx(tenant, id, &rows).await.is_err() {
            return Err("store error".to_string());
        }
        if let Some(a) = &alias {
            let _ = st.store.delete_alias(tenant, a).await;
        }
        st.cache.invalidate(id).await;
        st.webhooks.emit_if_in_memory(ev);
        return Ok(());
    }

    // add_tag / remove_tag / set_folder: all `LinkUpdated`.
    match op {
        BulkOp::AddTag => {
            // `normalize_tags` over the whole set is idempotent when the tag is
            // already present, and canonicalizes the new one (trim/lowercase).
            let mut tags = rec.tags.clone();
            tags.push(value.unwrap_or_default().to_string());
            rec.tags = normalize_tags(tags);
        }
        BulkOp::RemoveTag => {
            // Normalize the target the same way stored tags are, so it matches.
            if let Some(target) = normalize_tags(vec![value.unwrap_or_default().to_string()])
                .into_iter()
                .next()
            {
                rec.tags.retain(|t| t != &target);
            }
        }
        BulkOp::SetFolder => {
            // Empty/None clears the folder (`normalize_folder` maps it to None).
            rec.folder = normalize_folder(value.map(|s| s.to_string()));
        }
        BulkOp::Delete => unreachable!("delete handled above"),
    }

    let ev = WebhookEvent {
        event_type: EventType::LinkUpdated,
        body: webhook_event_payload(
            EventType::LinkUpdated,
            &canonical_code,
            &rec.url,
            alias.as_deref(),
            rec.expiry,
            rec.created,
            None,
        ),
        tenant_id: tenant,
    };
    let rows = st.webhooks.lifecycle_deliveries(tenant, &ev).await;
    if st.store.put_link_tx(tenant, id, &rec, &rows).await.is_err() {
        return Err("store error".to_string());
    }
    st.cache.invalidate(id).await;
    st.webhooks.emit_if_in_memory(ev);
    Ok(())
}

/// `POST /admin/links/bulk`: apply one operation (`delete` / `add_tag` /
/// `remove_tag` / `set_folder`) to a batch of links. Reuses the per-link
/// mutation primitives; a per-item failure (not found, store error) does not
/// abort the rest. Responds 200 with a partial report `{ ok, failed, results }`
/// in the spirit of the importer.
pub(crate) async fn admin_links_bulk(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let p = match admin_guard(&st, &headers, Scope::LinksWrite).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    let req: BulkReq = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid json").into_response(),
    };
    if req.codes.is_empty() {
        return (StatusCode::BAD_REQUEST, "codes must not be empty").into_response();
    }
    if req.codes.len() > MAX_BULK {
        return (StatusCode::BAD_REQUEST, "too many codes").into_response();
    }
    // `value` is required for the tag ops (an empty tag is meaningless). For
    // `set_folder`, a missing/empty value is allowed and means "remove from
    // folder"; `delete` ignores it.
    if matches!(req.op, BulkOp::AddTag | BulkOp::RemoveTag)
        && req.value.as_deref().is_none_or(|v| v.trim().is_empty())
    {
        return (StatusCode::BAD_REQUEST, "value required").into_response();
    }

    let mut results = Vec::with_capacity(req.codes.len());
    let mut ok = 0usize;
    let mut failed = 0usize;
    for code in &req.codes {
        match bulk_apply_one(&st, p.tenant, code, req.op, req.value.as_deref()).await {
            Ok(()) => {
                ok += 1;
                results.push(BulkItemResult {
                    code: code.clone(),
                    ok: true,
                    error: None,
                });
            }
            Err(e) => {
                failed += 1;
                results.push(BulkItemResult {
                    code: code.clone(),
                    ok: false,
                    error: Some(e),
                });
            }
        }
    }
    Json(BulkResp {
        ok,
        failed,
        results,
    })
    .into_response()
}
