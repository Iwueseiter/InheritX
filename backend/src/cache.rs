//! HTTP response caching primitives for InheritX.
//!
//! This module provides:
//! - ETag generation (SHA-256 of canonical JSON, Base64URL-encoded strong ETag)
//! - `If-None-Match` header parsing and ETag comparison
//! - `Cache-Control` header builders for public/private/no-store policies
//! - Helpers to build `304 Not Modified` and inject cache headers into `200` responses
//!
//! # Usage pattern in a GET handler
//!
//! ```rust,ignore
//! async fn get_something(
//!     State(state): State<Arc<AppState>>,
//!     headers: HeaderMap,
//!     // ...other extractors
//! ) -> Result<Response, ApiError> {
//!     let data = SomeService::fetch(&state.db, ...).await?;
//!
//!     let etag = cache::compute_etag(&data);
//!     if cache::is_not_modified(&headers, &etag) {
//!         return Ok(cache::not_modified_response(&etag));
//!     }
//!
//!     let body = Json(json!({ "status": "success", "data": data }));
//!     let mut response = body.into_response();
//!     cache::apply_cache_headers(&mut response, &etag, cache::cache_control_private(60));
//!     Ok(response)
//! }
//! ```

use axum::{
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::Serialize;
use sha2::{Digest, Sha256};

// ── ETag computation ──────────────────────────────────────────────────────────

/// Compute a strong ETag for any serializable value.
///
/// The ETag is the SHA-256 hash of the canonical JSON representation,
/// Base64URL-encoded (no padding), wrapped in double quotes as required
/// by RFC 7232: `"<hash>"`.
///
/// Returns the same ETag for the same data regardless of call order, making
/// it safe to use as a stable cache key.
pub fn compute_etag<T: Serialize>(data: &T) -> String {
    let json = serde_json::to_string(data).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(json.as_bytes());
    let hash = hasher.finalize();
    format!("\"{}\"", URL_SAFE_NO_PAD.encode(hash))
}

// ── Conditional-request helpers ───────────────────────────────────────────────

/// Extract the raw value of the `If-None-Match` request header, if present.
pub fn parse_if_none_match(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
}

/// Return `true` when the supplied ETag matches the client's `If-None-Match`
/// header value, meaning the cached response is still fresh.
///
/// Handles:
/// - Exact match: `"abc123" == "abc123"`
/// - Wildcard:    `If-None-Match: *` always matches
/// - Multi-value: `"abc123", "def456"` — matches if any value matches
pub fn etag_matches(etag: &str, if_none_match: &str) -> bool {
    let inm = if_none_match.trim();
    if inm == "*" {
        return true;
    }
    inm.split(',')
        .map(|s| s.trim())
        .any(|candidate| candidate == etag)
}

/// Convenience wrapper: return `true` when the request headers indicate the
/// response is still fresh (i.e. the ETag has not changed).
pub fn is_not_modified(headers: &HeaderMap, etag: &str) -> bool {
    match parse_if_none_match(headers) {
        Some(inm) => etag_matches(etag, &inm),
        None => false,
    }
}

// ── Cache-Control builders ────────────────────────────────────────────────────

/// `public, max-age=<seconds>, must-revalidate`
///
/// Use for responses that are safe to store in shared caches (CDN, proxies)
/// and that do not contain user-specific data.
pub fn cache_control_public(max_age_secs: u32) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "public, max-age={max_age_secs}, must-revalidate"
    ))
    .unwrap()
}

/// `private, max-age=<seconds>, must-revalidate`
///
/// Use for responses that contain user-specific data that must not be stored
/// in shared caches.
pub fn cache_control_private(max_age_secs: u32) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "private, max-age={max_age_secs}, must-revalidate"
    ))
    .unwrap()
}

/// `no-store`
///
/// Use for write-endpoint responses (POST / PUT / DELETE) and any endpoint
/// whose data must never be cached.
pub fn cache_control_no_store() -> HeaderValue {
    HeaderValue::from_static("no-store")
}

// ── Response builders ─────────────────────────────────────────────────────────

/// Build a `304 Not Modified` response with the given ETag.
///
/// Per RFC 7232 §4.1 the response MUST include `ETag` and SHOULD retain the
/// `Cache-Control` header from the original response so the client can update
/// its freshness information.
pub fn not_modified_response(etag: &str) -> Response {
    let etag_value = HeaderValue::from_str(etag).unwrap_or_else(|_| HeaderValue::from_static(""));
    (
        StatusCode::NOT_MODIFIED,
        [
            (header::ETAG, etag_value),
            (header::CACHE_CONTROL, cache_control_private(60)),
        ],
    )
        .into_response()
}

/// Build a `304 Not Modified` response with a custom `Cache-Control` header.
pub fn not_modified_response_with_cc(etag: &str, cache_control: HeaderValue) -> Response {
    let etag_value = HeaderValue::from_str(etag).unwrap_or_else(|_| HeaderValue::from_static(""));
    (
        StatusCode::NOT_MODIFIED,
        [
            (header::ETAG, etag_value),
            (header::CACHE_CONTROL, cache_control),
        ],
    )
        .into_response()
}

/// Inject `ETag`, `Cache-Control`, and `Vary: Accept-Encoding` headers into
/// an existing `200 OK` response.
///
/// This mutates the response in-place so the handler can build its response
/// normally and then call this as a final decoration step.
pub fn apply_cache_headers(response: &mut Response, etag: &str, cache_control: HeaderValue) {
    let headers = response.headers_mut();
    if let Ok(etag_value) = HeaderValue::from_str(etag) {
        headers.insert(header::ETAG, etag_value);
    }
    headers.insert(header::CACHE_CONTROL, cache_control);
    // Vary: Accept-Encoding ensures compressed vs. uncompressed responses are
    // stored separately in any intermediary cache.
    headers.insert(
        header::VARY,
        HeaderValue::from_static("Accept-Encoding"),
    );
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn etag_is_deterministic() {
        let data = json!({ "id": 1, "name": "test" });
        let e1 = compute_etag(&data);
        let e2 = compute_etag(&data);
        assert_eq!(e1, e2, "same data must produce the same ETag");
        assert!(e1.starts_with('"') && e1.ends_with('"'));
    }

    #[test]
    fn different_data_produces_different_etag() {
        let a = json!({ "id": 1 });
        let b = json!({ "id": 2 });
        assert_ne!(compute_etag(&a), compute_etag(&b));
    }

    #[test]
    fn etag_matches_exact() {
        let etag = compute_etag(&json!({ "x": 42 }));
        assert!(etag_matches(&etag, &etag));
    }

    #[test]
    fn etag_matches_wildcard() {
        let etag = compute_etag(&json!({ "x": 42 }));
        assert!(etag_matches(&etag, "*"));
    }

    #[test]
    fn etag_no_match_on_stale() {
        let etag_new = compute_etag(&json!({ "x": 42 }));
        let etag_old = compute_etag(&json!({ "x": 1 }));
        assert!(!etag_matches(&etag_new, &etag_old));
    }

    #[test]
    fn etag_matches_multi_value() {
        let etag = compute_etag(&json!({ "x": 42 }));
        let stale = compute_etag(&json!({ "x": 1 }));
        let header = format!("{stale}, {etag}");
        assert!(etag_matches(&etag, &header));
    }

    #[test]
    fn is_not_modified_true_on_match() {
        let data = json!({ "id": 99 });
        let etag = compute_etag(&data);
        let mut headers = HeaderMap::new();
        headers.insert(
            header::IF_NONE_MATCH,
            HeaderValue::from_str(&etag).unwrap(),
        );
        assert!(is_not_modified(&headers, &etag));
    }

    #[test]
    fn is_not_modified_false_when_no_header() {
        let etag = compute_etag(&json!({ "id": 99 }));
        let headers = HeaderMap::new();
        assert!(!is_not_modified(&headers, &etag));
    }

    #[test]
    fn cache_control_public_format() {
        let v = cache_control_public(300);
        assert_eq!(v.to_str().unwrap(), "public, max-age=300, must-revalidate");
    }

    #[test]
    fn cache_control_private_format() {
        let v = cache_control_private(60);
        assert_eq!(v.to_str().unwrap(), "private, max-age=60, must-revalidate");
    }

    #[test]
    fn cache_control_no_store_format() {
        let v = cache_control_no_store();
        assert_eq!(v.to_str().unwrap(), "no-store");
    }

    #[test]
    fn apply_cache_headers_injects_all_three() {
        let mut response = StatusCode::OK.into_response();
        let etag = compute_etag(&json!({ "v": 1 }));
        apply_cache_headers(&mut response, &etag, cache_control_private(120));

        let hdrs = response.headers();
        assert!(hdrs.contains_key(header::ETAG));
        assert!(hdrs.contains_key(header::CACHE_CONTROL));
        assert!(hdrs.contains_key(header::VARY));
    }
}
