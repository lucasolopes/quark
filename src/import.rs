//! Parsing for the bulk migration importer (`POST /admin/import`).
//!
//! Accepts either a JSON array of link objects or a CSV export (Bitly /
//! Kutt / YOURLS style) and normalizes both into `ImportRow`. Pure parsing:
//! no store access, no HTTP; `src/api.rs` drives each row through
//! `create_link_core`.

use serde::Deserialize;

/// Hard cap on rows accepted per `POST /admin/import` request, to bound
/// memory and synchronous runtime (the endpoint has no background job queue).
pub const MAX_IMPORT_ROWS: usize = 10_000;

/// A single row to import: destination URL, optional custom alias, optional
/// TTL in seconds. Field aliases cover common export vocabularies (Bitly's
/// `long_url`/`longUrl`, YOURLS' `keyword`).
#[derive(Debug, Deserialize, PartialEq, Eq, Clone)]
pub struct ImportRow {
    #[serde(alias = "long_url", alias = "longUrl")]
    pub url: String,
    #[serde(default, alias = "keyword", alias = "short")]
    pub alias: Option<String>,
    #[serde(default)]
    pub ttl: Option<u64>,
}

/// Body format for an import request, resolved from `Content-Type` with a
/// content-sniffing fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportFormat {
    Json,
    Csv,
}

/// Chooses JSON or CSV from the `Content-Type` header; when absent or
/// unrecognized, sniffs the first non-whitespace byte of the body (`[` or
/// `{` implies JSON, anything else is treated as CSV).
pub fn detect_format(content_type: Option<&str>, body: &[u8]) -> ImportFormat {
    if let Some(ct) = content_type {
        let ct = ct.to_ascii_lowercase();
        if ct.contains("application/json") {
            return ImportFormat::Json;
        }
        if ct.contains("text/csv") || ct.contains("application/csv") {
            return ImportFormat::Csv;
        }
    }
    match body.iter().find(|b| !b.is_ascii_whitespace()) {
        Some(b'[') | Some(b'{') => ImportFormat::Json,
        _ => ImportFormat::Csv,
    }
}

/// Why parsing an import body failed. Kept coarse: the caller only needs to
/// map this to a 400 response, not surface fine-grained detail.
#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    InvalidJson,
    InvalidCsv,
    MissingUrlColumn,
}

/// Parses the raw request body into `ImportRow`s per `format`.
pub fn import_rows(bytes: &[u8], format: ImportFormat) -> Result<Vec<ImportRow>, ParseError> {
    match format {
        ImportFormat::Json => {
            serde_json::from_slice::<Vec<ImportRow>>(bytes).map_err(|_| ParseError::InvalidJson)
        }
        ImportFormat::Csv => parse_csv(bytes),
    }
}

const URL_COLUMNS: &[&str] = &["url", "long_url", "longurl", "original_url", "long"];
const ALIAS_COLUMNS: &[&str] = &["alias", "keyword", "short", "short_code", "custom"];
const TTL_COLUMNS: &[&str] = &["ttl", "expiry"];

/// Finds the index of the first header matching (case-insensitively) any of
/// `names`.
fn find_column(headers: &csv::StringRecord, names: &[&str]) -> Option<usize> {
    headers
        .iter()
        .position(|h| names.contains(&h.trim().to_ascii_lowercase().as_str()))
}

fn parse_csv(bytes: &[u8]) -> Result<Vec<ImportRow>, ParseError> {
    let mut reader = csv::ReaderBuilder::new().flexible(true).from_reader(bytes);
    let headers = reader
        .headers()
        .map_err(|_| ParseError::InvalidCsv)?
        .clone();
    let url_idx = find_column(&headers, URL_COLUMNS).ok_or(ParseError::MissingUrlColumn)?;
    let alias_idx = find_column(&headers, ALIAS_COLUMNS);
    let ttl_idx = find_column(&headers, TTL_COLUMNS);

    let mut rows = Vec::new();
    for result in reader.records() {
        let record = result.map_err(|_| ParseError::InvalidCsv)?;
        let url = record.get(url_idx).unwrap_or("").trim().to_string();
        let alias = alias_idx
            .and_then(|i| record.get(i))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let ttl = ttl_idx
            .and_then(|i| record.get(i))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(|s| s.parse::<u64>().ok());
        rows.push(ImportRow { url, alias, ttl });
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_json_from_content_type() {
        assert_eq!(
            detect_format(Some("application/json"), b""),
            ImportFormat::Json
        );
    }

    #[test]
    fn detects_csv_from_content_type() {
        assert_eq!(detect_format(Some("text/csv"), b""), ImportFormat::Csv);
    }

    #[test]
    fn sniffs_json_array_without_content_type() {
        assert_eq!(
            detect_format(None, b"  [{\"url\":\"x\"}]"),
            ImportFormat::Json
        );
    }

    #[test]
    fn sniffs_csv_without_content_type() {
        assert_eq!(
            detect_format(None, b"url\nhttps://a.com"),
            ImportFormat::Csv
        );
    }

    #[test]
    fn parses_json_array_with_field_aliases() {
        let body = br#"[{"long_url":"https://a.com","keyword":"promo","ttl":60}]"#;
        let rows = import_rows(body, ImportFormat::Json).unwrap();
        assert_eq!(
            rows,
            vec![ImportRow {
                url: "https://a.com".to_string(),
                alias: Some("promo".to_string()),
                ttl: Some(60),
            }]
        );
    }

    #[test]
    fn parses_csv_with_yourls_style_header() {
        let body = b"keyword,url\npromo,https://a.com\n";
        let rows = import_rows(body, ImportFormat::Csv).unwrap();
        assert_eq!(
            rows,
            vec![ImportRow {
                url: "https://a.com".to_string(),
                alias: Some("promo".to_string()),
                ttl: None,
            }]
        );
    }

    #[test]
    fn csv_without_url_column_is_an_error() {
        let body = b"alias\npromo\n";
        assert_eq!(
            import_rows(body, ImportFormat::Csv),
            Err(ParseError::MissingUrlColumn)
        );
    }

    #[test]
    fn invalid_json_is_an_error() {
        assert_eq!(
            import_rows(b"not json", ImportFormat::Json),
            Err(ParseError::InvalidJson)
        );
    }
}
