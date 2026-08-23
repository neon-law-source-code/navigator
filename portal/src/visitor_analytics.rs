//! Privacy-preserving public website visit counter.
//!
//! This records bounded aggregate counters only. It never reads or records IP
//! addresses, user agents, sessions, raw query strings, or full referrer URLs.

use std::sync::Arc;

use axum::extract::{MatchedPath, Request, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use store::surreal::SurrealDb;
use store::visitor_analytics::VisitorVisit;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const COUNTRY_HEADER: &str = "x-navigator-client-region";
const UNKNOWN_COUNTRY: &str = "ZZ";
const DIRECT_SOURCE: &str = "direct";
const INTERNAL_SOURCE: &str = "internal";
const OTHER_SOURCE: &str = "other";
const INVALID_SOURCE: &str = "invalid";
const MAX_SOURCE_LEN: usize = 64;
const ALLOWED_QUERY_SOURCE_KEYS: &[&str] = &["utm_source", "ref", "utm_medium", "utm_campaign"];
/// The most aggregate analytics writes allowed in flight at once. Past this
/// bound a public request drops its best-effort write instead of spawning an
/// unbounded task that would pile up waiting on a slow or exhausted pool.
const MAX_INFLIGHT_ANALYTICS_WRITES: usize = 64;
const COUNTRY_CODES: &[&str] = &[
    "AD", "AE", "AF", "AG", "AI", "AL", "AM", "AO", "AQ", "AR", "AS", "AT", "AU", "AW", "AX", "AZ",
    "BA", "BB", "BD", "BE", "BF", "BG", "BH", "BI", "BJ", "BL", "BM", "BN", "BO", "BQ", "BR", "BS",
    "BT", "BV", "BW", "BY", "BZ", "CA", "CC", "CD", "CF", "CG", "CH", "CI", "CK", "CL", "CM", "CN",
    "CO", "CR", "CU", "CV", "CW", "CX", "CY", "CZ", "DE", "DJ", "DK", "DM", "DO", "DZ", "EC", "EE",
    "EG", "EH", "ER", "ES", "ET", "FI", "FJ", "FK", "FM", "FO", "FR", "GA", "GB", "GD", "GE", "GF",
    "GG", "GH", "GI", "GL", "GM", "GN", "GP", "GQ", "GR", "GS", "GT", "GU", "GW", "GY", "HK", "HM",
    "HN", "HR", "HT", "HU", "ID", "IE", "IL", "IM", "IN", "IO", "IQ", "IR", "IS", "IT", "JE", "JM",
    "JO", "JP", "KE", "KG", "KH", "KI", "KM", "KN", "KP", "KR", "KW", "KY", "KZ", "LA", "LB", "LC",
    "LI", "LK", "LR", "LS", "LT", "LU", "LV", "LY", "MA", "MC", "MD", "ME", "MF", "MG", "MH", "MK",
    "ML", "MM", "MN", "MO", "MP", "MQ", "MR", "MS", "MT", "MU", "MV", "MW", "MX", "MY", "MZ", "NA",
    "NC", "NE", "NF", "NG", "NI", "NL", "NO", "NP", "NR", "NU", "NZ", "OM", "PA", "PE", "PF", "PG",
    "PH", "PK", "PL", "PM", "PN", "PR", "PS", "PT", "PW", "PY", "QA", "RE", "RO", "RS", "RU", "RW",
    "SA", "SB", "SC", "SD", "SE", "SG", "SH", "SI", "SJ", "SK", "SL", "SM", "SN", "SO", "SR", "SS",
    "ST", "SV", "SX", "SY", "SZ", "TC", "TD", "TF", "TG", "TH", "TJ", "TK", "TL", "TM", "TN", "TO",
    "TR", "TT", "TV", "TW", "TZ", "UA", "UG", "UM", "US", "UY", "UZ", "VA", "VC", "VE", "VG", "VI",
    "VN", "VU", "WF", "WS", "YE", "YT", "ZA", "ZM", "ZW",
];

/// Middleware state for [`count_public_visit`]: the database handle plus a
/// bound on how many aggregate writes may be in flight at once.
#[derive(Clone)]
pub struct VisitorAnalyticsState {
    db: SurrealDb,
    write_slots: Arc<Semaphore>,
}

impl VisitorAnalyticsState {
    /// Build the state with the default in-flight write bound.
    #[must_use]
    pub fn new(db: SurrealDb) -> Self {
        Self {
            db,
            write_slots: Arc::new(Semaphore::new(MAX_INFLIGHT_ANALYTICS_WRITES)),
        }
    }
}

/// Reserve one in-flight-write slot, or `None` when the bound is reached and the
/// best-effort write should be dropped rather than queued behind a slow pool.
/// The returned permit releases its slot when the spawned write task drops it.
fn reserve_write_slot(slots: &Arc<Semaphore>) -> Option<OwnedSemaphorePermit> {
    Arc::clone(slots).try_acquire_owned().ok()
}

/// Count public website visits after the handler returns so the metric can use
/// a coarse status class. Filtering happens before `next` from request metadata
/// only; no IP-bearing headers are inspected.
pub async fn count_public_visit(
    State(state): State<VisitorAnalyticsState>,
    req: Request,
    next: Next,
) -> Response {
    let method = req.method().clone();
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str)
        .map(str::to_string);
    let query = req.uri().query().map(str::to_string);
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let referer = req
        .headers()
        .get(header::REFERER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let country = country_from_headers(req.headers());

    let should_count = route
        .as_deref()
        .is_some_and(|route| should_count_route(&method, route));

    let response = next.run(req).await;

    if should_count {
        let route = route.expect("route checked above");
        let source = source_from_request(query.as_deref(), referer.as_deref(), host.as_deref());
        // Navigator publishes one language, so this dimension is constant.
        // It stays on the metric rather than being dropped, because removing a
        // label breaks the dashboards and BigQuery views already reading it.
        let locale = "en";
        let status_class = status_class(response.status());
        telemetry::record_web_visit(&route, &country, &source, locale, status_class);
        persist_or_drop(
            &state,
            RecordedVisit {
                country,
                route,
                source,
                locale,
                status_class,
            },
        );
    }

    response
}

/// One counted visit, owned so it can move into the spawned write task.
struct RecordedVisit {
    country: String,
    route: String,
    source: String,
    locale: &'static str,
    status_class: &'static str,
}

/// Persist a counted visit off the request's critical path, bounded so a slow
/// or exhausted pool can't grow an unbounded backlog of in-flight writes. Past
/// the bound the best-effort counter is dropped rather than queued; within it,
/// failures are logged, never surfaced to the visitor.
fn persist_or_drop(state: &VisitorAnalyticsState, visit: RecordedVisit) {
    let Some(permit) = reserve_write_slot(&state.write_slots) else {
        tracing::warn!(route = %visit.route, "visitor analytics write dropped at in-flight bound");
        return;
    };
    let db = state.db.clone();
    tokio::spawn(async move {
        let _permit = permit;
        if let Err(error) = store::visitor_analytics::record_visit(
            &db,
            &VisitorVisit {
                country_code: &visit.country,
                route_pattern: &visit.route,
                source: &visit.source,
                locale: visit.locale,
                status_class: visit.status_class,
            },
        )
        .await
        {
            tracing::warn!(%error, route = %visit.route, "visitor analytics aggregate write failed");
        }
    });
}

fn should_count_route(method: &Method, route: &str) -> bool {
    if method != Method::GET && method != Method::HEAD {
        return false;
    }

    !is_excluded_route(route)
}

fn is_excluded_route(route: &str) -> bool {
    route == "/health"
        || route == "/readyz"
        || route == "/version"
        || route.starts_with("/app")
        || route.starts_with("/admin")
        || route.starts_with("/lawyer")
        || route.starts_with("/auth")
        || route.starts_with("/mcp")
        || route.starts_with("/webhook")
        || route.starts_with("/webhooks")
        || route.starts_with("/docusign")
        || route.starts_with("/projects")
        || route.starts_with("/public")
        || route.starts_with("/git")
}

fn country_from_headers(headers: &HeaderMap) -> String {
    headers
        .get(COUNTRY_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .map(str::to_ascii_uppercase)
        .filter(|value| is_country_bucket(value))
        .unwrap_or_else(|| UNKNOWN_COUNTRY.to_string())
}

fn is_country_bucket(value: &str) -> bool {
    COUNTRY_CODES.binary_search(&value).is_ok()
}

fn source_from_request(query: Option<&str>, referer: Option<&str>, host: Option<&str>) -> String {
    source_from_query(query).unwrap_or_else(|| source_from_referer(referer, host))
}

fn source_from_query(query: Option<&str>) -> Option<String> {
    let query = query?;

    // Attribute by a stable precedence over the allowed keys, not by their
    // position in the query string, so equivalent campaign URLs record the
    // same source regardless of parameter ordering.
    for allowed in ALLOWED_QUERY_SOURCE_KEYS {
        for pair in query.split('&') {
            let Some((key, value)) = pair.split_once('=') else {
                continue;
            };
            if key == *allowed {
                return Some(decode_query_value(value).map_or_else(
                    || INVALID_SOURCE.to_string(),
                    |decoded| normalize_source(&decoded),
                ));
            }
        }
    }

    None
}

fn normalize_source(raw: &str) -> String {
    let normalized = raw
        .trim()
        .split_ascii_whitespace()
        .collect::<Vec<_>>()
        .join("-");

    if normalized.is_empty()
        || normalized.len() > MAX_SOURCE_LEN
        || normalized.contains('@')
        || !normalized.bytes().all(is_safe_source_byte)
    {
        return INVALID_SOURCE.to_string();
    }

    normalized.to_ascii_lowercase()
}

fn source_from_referer(referer: Option<&str>, current_host: Option<&str>) -> String {
    let Some(referer) = referer.map(str::trim).filter(|value| !value.is_empty()) else {
        return DIRECT_SOURCE.to_string();
    };
    let Some(host) = host_from_url(referer) else {
        return OTHER_SOURCE.to_string();
    };
    let host = strip_www(&host);
    if current_host
        .and_then(normalize_host)
        .is_some_and(|current| same_site(&host, &current))
    {
        return INTERNAL_SOURCE.to_string();
    }
    classify_external_host(&host).to_string()
}

fn host_from_url(raw: &str) -> Option<String> {
    let rest = raw
        .strip_prefix("https://")
        .or_else(|| raw.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next()?.trim();
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let host = authority
        .rsplit_once(':')
        .map_or(authority, |(host, port)| {
            if port.bytes().all(|b| b.is_ascii_digit()) {
                host
            } else {
                authority
            }
        })
        .trim_matches(['[', ']'])
        .to_ascii_lowercase();
    if host.is_empty() {
        None
    } else {
        Some(host)
    }
}

fn normalize_host(raw: &str) -> Option<String> {
    let host = raw
        .split(',')
        .next()
        .unwrap_or(raw)
        .trim()
        .rsplit_once(':')
        .map_or(raw.trim(), |(host, port)| {
            if port.bytes().all(|b| b.is_ascii_digit()) {
                host
            } else {
                raw.trim()
            }
        })
        .trim_matches(['[', ']'])
        .to_ascii_lowercase();
    if host.is_empty() {
        None
    } else {
        Some(strip_www(&host))
    }
}

fn strip_www(host: &str) -> String {
    host.strip_prefix("www.").unwrap_or(host).to_string()
}

fn same_site(referer_host: &str, current_host: &str) -> bool {
    referer_host == current_host || referer_host.ends_with(&format!(".{current_host}"))
}

fn classify_external_host(host: &str) -> &'static str {
    if host == "linkedin.com" || host.ends_with(".linkedin.com") {
        "linkedin"
    } else if host == "google.com" || host.ends_with(".google.com") {
        "google"
    } else if host == "bing.com" || host.ends_with(".bing.com") {
        "bing"
    } else if host == "facebook.com" || host.ends_with(".facebook.com") {
        "facebook"
    } else if host == "instagram.com" || host.ends_with(".instagram.com") {
        "instagram"
    } else if host == "x.com"
        || host.ends_with(".x.com")
        || host == "twitter.com"
        || host.ends_with(".twitter.com")
    {
        "social"
    } else {
        OTHER_SOURCE
    }
}

fn decode_query_value(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' => {
                let high = *bytes.get(index + 1)?;
                let low = *bytes.get(index + 2)?;
                decoded.push((hex_value(high)? << 4) | hex_value(low)?);
                index += 3;
            }
            _ => {
                decoded.push(byte);
                index += 1;
            }
        }
    }

    String::from_utf8(decoded).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn is_safe_source_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
}

fn status_class(status: StatusCode) -> &'static str {
    match status.as_u16() {
        100..=199 => "1xx",
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        500..=599 => "5xx",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        country_from_headers, persist_or_drop, reserve_write_slot, should_count_route,
        source_from_referer, source_from_request, status_class, Arc, HeaderMap, Method,
        RecordedVisit, Semaphore, StatusCode, VisitorAnalyticsState,
    };

    fn sample_visit() -> RecordedVisit {
        RecordedVisit {
            country: "US".to_string(),
            route: "/blog/{slug}".to_string(),
            source: "linkedin".to_string(),
            locale: "en",
            status_class: "2xx",
        }
    }

    #[test]
    fn analytics_writes_drop_once_the_in_flight_bound_is_reached() {
        let slots = Arc::new(Semaphore::new(2));
        let first = reserve_write_slot(&slots).expect("first slot reserved");
        let _second = reserve_write_slot(&slots).expect("second slot reserved");
        assert!(
            reserve_write_slot(&slots).is_none(),
            "writes past the in-flight bound are dropped"
        );
        drop(first);
        assert!(
            reserve_write_slot(&slots).is_some(),
            "a freed slot is reusable by the next write"
        );
    }

    #[tokio::test]
    async fn saturated_and_failing_writes_log_without_panicking() {
        // No free slots: the write is dropped and logged, never spawned.
        let saturated = VisitorAnalyticsState {
            db: store::surreal::test_support::mem().await,
            write_slots: Arc::new(Semaphore::new(0)),
        };
        persist_or_drop(&saturated, sample_visit());

        // A free slot but an engine that cannot answer: the spawned write
        // errors and logs, and the request path stays unaffected.
        let live = VisitorAnalyticsState::new(store::surreal::test_support::unreachable());
        persist_or_drop(&live, sample_visit());
        // Let the fire-and-forget task run its failing write and log.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    #[test]
    fn allowed_query_source_params_are_retained_in_normalized_form() {
        assert_eq!(
            source_from_request(Some("utm_source=LinkedIn+Ads"), None, None),
            "linkedin-ads"
        );
        assert_eq!(
            source_from_request(Some("ref=Newsletter.1"), None, None),
            "newsletter.1"
        );
        assert_eq!(
            source_from_request(Some("utm_source=linkedin&ref=newsletter"), None, None),
            "linkedin"
        );
        assert_eq!(
            source_from_request(Some("utm_medium=paid%2Dsearch"), None, None),
            "paid-search"
        );
    }

    #[test]
    fn query_source_attribution_is_independent_of_parameter_order() {
        // `utm_source` outranks `ref` regardless of which appears first, so
        // equivalent campaign URLs attribute to the same source.
        assert_eq!(
            source_from_request(Some("utm_source=linkedin&ref=newsletter"), None, None),
            "linkedin"
        );
        assert_eq!(
            source_from_request(Some("ref=newsletter&utm_source=linkedin"), None, None),
            "linkedin"
        );
        // `ref` outranks the lower-precedence `utm_medium`/`utm_campaign`.
        assert_eq!(
            source_from_request(Some("utm_campaign=spring&ref=newsletter"), None, None),
            "newsletter"
        );
    }

    #[test]
    fn referrer_hosts_map_to_the_full_bounded_bucket_set() {
        let here = Some("neonlaw.com");
        assert_eq!(
            source_from_referer(Some("https://www.bing.com/search?q=x"), here),
            "bing"
        );
        assert_eq!(
            source_from_referer(Some("https://m.facebook.com/story"), here),
            "facebook"
        );
        assert_eq!(
            source_from_referer(Some("https://instagram.com/neonlaw"), here),
            "instagram"
        );
        assert_eq!(
            source_from_referer(Some("https://x.com/neonlaw"), here),
            "social"
        );
        assert_eq!(
            source_from_referer(Some("https://twitter.com/neonlaw"), here),
            "social"
        );
        assert_eq!(
            source_from_referer(Some("https://news.ycombinator.com/item?id=1"), here),
            "other"
        );
    }

    #[test]
    fn referrer_host_parsing_handles_ports_userinfo_and_subdomains() {
        // A port on either side is stripped before the same-site comparison.
        assert_eq!(
            source_from_referer(
                Some("https://neonlaw.com:443/about"),
                Some("neonlaw.com:8080")
            ),
            "internal"
        );
        // A subdomain of the current host still counts as internal.
        assert_eq!(
            source_from_referer(Some("https://blog.neonlaw.com/post"), Some("neonlaw.com")),
            "internal"
        );
        // A userinfo `@` authority is rejected as an unparseable referrer.
        assert_eq!(
            source_from_referer(Some("https://user@evil.example/x"), Some("neonlaw.com")),
            "other"
        );
        // A non-http(s) scheme is not a host we classify.
        assert_eq!(
            source_from_referer(
                Some("android-app://com.linkedin.android"),
                Some("neonlaw.com")
            ),
            "other"
        );
        // Whitespace-only referrer is treated as a direct visit.
        assert_eq!(
            source_from_referer(Some("   "), Some("neonlaw.com")),
            "direct"
        );
    }

    #[test]
    fn unknown_sensitive_blank_and_long_query_values_collapse_to_safe_buckets() {
        assert_eq!(
            source_from_request(Some("token=abc123"), None, None),
            "direct"
        );
        assert_eq!(
            source_from_request(Some("email=nick@example.com"), None, None),
            "direct"
        );
        assert_eq!(
            source_from_request(Some("utm_source="), None, None),
            "invalid"
        );
        assert_eq!(
            source_from_request(Some("utm_source=nick@example.com"), None, None),
            "invalid"
        );
        assert_eq!(
            source_from_request(
                Some(
                    "utm_source=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                ),
                None,
                None
            ),
            "invalid"
        );
        assert_eq!(
            source_from_request(Some("utm_source=https://linkedin.com/in/user"), None, None),
            "invalid"
        );
        assert_eq!(
            source_from_request(Some("utm_source=bad%ZZvalue"), None, None),
            "invalid"
        );
    }

    #[test]
    fn referrer_host_parsing_handles_non_numeric_ports_and_empty_authorities() {
        // A non-numeric ":port" on the referer is not stripped, so the host
        // keeps it and can't match the current site — it stays external.
        assert_eq!(
            source_from_referer(Some("https://neonlaw.com:abc/x"), Some("neonlaw.com")),
            "other"
        );
        // A non-numeric ":port" on the current host likewise isn't stripped, so
        // a bare referer host does not count as internal.
        assert_eq!(
            source_from_referer(Some("https://neonlaw.com/x"), Some("neonlaw.com:abc")),
            "other"
        );
        // An authority that is only a port yields an empty host — an
        // unparseable referer, classified as "other".
        assert_eq!(
            source_from_referer(Some("https://:443/path"), Some("neonlaw.com")),
            "other"
        );
        // A current host that normalizes to nothing means nothing is internal.
        assert_eq!(
            source_from_referer(Some("https://neonlaw.com/x"), Some(":80")),
            "other"
        );
    }

    #[test]
    fn referrer_classification_uses_bounded_host_buckets_only() {
        assert_eq!(
            source_from_referer(
                Some("https://www.linkedin.com/feed/update/123?tracking=abc"),
                Some("neonlaw.com")
            ),
            "linkedin"
        );
        assert_eq!(
            source_from_referer(
                Some("https://neonlaw.com/blog/welcome?x=y"),
                Some("neonlaw.com")
            ),
            "internal"
        );
        assert_eq!(
            source_from_referer(
                Some("https://google.com/search?q=neon"),
                Some("neonlaw.com")
            ),
            "google"
        );
        assert_eq!(source_from_referer(None, Some("neonlaw.com")), "direct");
        assert_eq!(
            source_from_referer(Some("not a url"), Some("neonlaw.com")),
            "other"
        );
    }

    #[test]
    fn country_header_is_bounded_and_never_ip_derived() {
        let mut headers = HeaderMap::new();
        assert_eq!(country_from_headers(&headers), "ZZ");

        headers.insert("x-navigator-client-region", "us".parse().unwrap());
        assert_eq!(country_from_headers(&headers), "US");

        headers.insert("x-navigator-client-region", "ca".parse().unwrap());
        assert_eq!(country_from_headers(&headers), "CA");

        headers.insert("x-navigator-client-region", "AA".parse().unwrap());
        assert_eq!(country_from_headers(&headers), "ZZ");

        headers.insert("x-navigator-client-region", "A00".parse().unwrap());
        assert_eq!(country_from_headers(&headers), "ZZ");

        headers.insert("x-navigator-client-region", "203.0.113.7".parse().unwrap());
        assert_eq!(country_from_headers(&headers), "ZZ");
    }

    #[test]
    fn only_public_get_and_head_routes_are_counted() {
        assert!(should_count_route(&Method::GET, "/team"));
        assert!(should_count_route(&Method::HEAD, "/notations"));
        assert!(!should_count_route(&Method::POST, "/contact"));
        assert!(!should_count_route(&Method::GET, "/admin"));
        assert!(!should_count_route(&Method::GET, "/lawyer"));
        assert!(!should_count_route(&Method::GET, "/app/projects"));
        assert!(!should_count_route(&Method::GET, "/app/forms"));
        assert!(!should_count_route(&Method::GET, "/app/api/aida.json"));
        assert!(!should_count_route(&Method::GET, "/app/api/people"));
        assert!(!should_count_route(&Method::GET, "/health"));
        assert!(!should_count_route(&Method::GET, "/public/app.css"));
        assert!(!should_count_route(&Method::GET, "/webhooks/sendgrid"));
        assert!(!should_count_route(&Method::GET, "/git/acme.git/info/refs"));
    }

    #[test]
    fn status_is_reduced_to_class() {
        assert_eq!(status_class(StatusCode::OK), "2xx");
        assert_eq!(status_class(StatusCode::PERMANENT_REDIRECT), "3xx");
        assert_eq!(status_class(StatusCode::NOT_FOUND), "4xx");
        assert_eq!(status_class(StatusCode::INTERNAL_SERVER_ERROR), "5xx");
    }
}
