//! Per-IP request rate limiting for the abuse-sensitive endpoints.
//!
//! A small in-process fixed-window limiter keyed on the client IP. It is
//! the application-layer backstop for credential stuffing against
//! `/auth/*` and floods against the LLM-backed agent endpoints
//! (`/mcp`, `/app/api/mcp/rpc`) — the edge (Cloud Armor on the GKE Gateway)
//! is the first line, this is defense in depth that travels with the
//! binary so an OSS fork without an edge WAF still has a floor.
//!
//! Fixed-window, not token-bucket: simpler, allocation-free per request,
//! and good enough for "stop a brute-force script." Counters live in a
//! per-pod `Mutex<HashMap>`, so the effective limit is `max * replicas` —
//! acceptable for a backstop, and the edge handles the real ceiling.
//!
//! Disabled by default ([`RateLimit::disabled`]) so tests and dev pay
//! nothing; `from_env` turns it on in production via
//! `NAVIGATOR_RATE_LIMIT_PER_MIN`.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

/// Default per-IP budget per minute when `NAVIGATOR_RATE_LIMIT_PER_MIN`
/// is unset but rate limiting is otherwise enabled. Generous enough that
/// an interactive user or a well-behaved agent never trips it, low enough
/// to throttle a brute-force loop.
pub const DEFAULT_PER_MINUTE: u32 = 60;

/// Cheaply-cloneable handle to the shared limiter state.
#[derive(Clone)]
pub struct RateLimit(Arc<Inner>);

struct Inner {
    enabled: bool,
    max: u32,
    window: Duration,
    buckets: Mutex<HashMap<IpAddr, Bucket>>,
}

#[derive(Clone, Copy)]
struct Bucket {
    window_start: Instant,
    count: u32,
}

impl RateLimit {
    /// An enabled limiter allowing `max` requests per `window` per IP.
    #[must_use]
    pub fn new(max: u32, window: Duration) -> Self {
        Self(Arc::new(Inner {
            enabled: true,
            max,
            window,
            buckets: Mutex::new(HashMap::new()),
        }))
    }

    /// A no-op limiter — every request is allowed. Used by tests and the
    /// dev defaults so request-heavy suites never throttle themselves.
    #[must_use]
    pub fn disabled() -> Self {
        Self(Arc::new(Inner {
            enabled: false,
            max: 0,
            window: Duration::from_mins(1),
            buckets: Mutex::new(HashMap::new()),
        }))
    }

    /// Build from the environment. `NAVIGATOR_RATE_LIMIT_PER_MIN=0` (or an
    /// unparseable value) yields a disabled limiter; any positive integer
    /// enables a per-minute budget. Unset defaults to enabled at
    /// [`DEFAULT_PER_MINUTE`] so production has a floor without extra
    /// config.
    #[must_use]
    pub fn from_env() -> Self {
        match std::env::var("NAVIGATOR_RATE_LIMIT_PER_MIN") {
            Ok(raw) => match raw.trim().parse::<u32>() {
                Ok(0) | Err(_) => Self::disabled(),
                Ok(max) => Self::new(max, Duration::from_mins(1)),
            },
            Err(_) => Self::new(DEFAULT_PER_MINUTE, Duration::from_mins(1)),
        }
    }

    /// Record a request from `ip` at `now` and report whether it is within
    /// budget. Taking `now` explicitly keeps the window logic unit-testable
    /// without sleeping.
    fn check(&self, ip: IpAddr, now: Instant) -> bool {
        if !self.0.enabled {
            return true;
        }
        let mut buckets = self.0.buckets.lock().expect("rate-limit mutex poisoned");
        let bucket = buckets.entry(ip).or_insert(Bucket {
            window_start: now,
            count: 0,
        });
        if now.duration_since(bucket.window_start) >= self.0.window {
            // Window elapsed → reset.
            bucket.window_start = now;
            bucket.count = 0;
        }
        bucket.count += 1;
        bucket.count <= self.0.max
    }
}

/// Extract the client IP. Behind the GKE Gateway / Cloud Armor the real
/// client is the left-most entry of `X-Forwarded-For`; we fall back to
/// the direct peer when present and finally to an unspecified address so
/// a missing header degrades to a single shared bucket rather than
/// bypassing the limit.
fn client_ip(req: &Request) -> IpAddr {
    if let Some(xff) = req
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
    {
        if let Some(first) = xff.split(',').next() {
            if let Ok(ip) = first.trim().parse::<IpAddr>() {
                return ip;
            }
        }
    }
    if let Some(peer) = req
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
    {
        return peer.0.ip();
    }
    IpAddr::from([0, 0, 0, 0])
}

/// Axum middleware: 429 once an IP exceeds its budget in the current
/// window. A pass-through when the limiter is disabled.
pub async fn enforce(
    State(rl): State<RateLimit>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if !rl.0.enabled {
        return Ok(next.run(req).await);
    }
    let ip = client_ip(&req);
    if rl.check(ip, Instant::now()) {
        return Ok(next.run(req).await);
    }
    tracing::warn!(
        target: "audit",
        event = "rate_limit.exceeded",
        ip = %ip,
        "rate limit: per-IP budget exceeded; returning 429",
    );
    let mut resp = Response::new(axum::body::Body::empty());
    *resp.status_mut() = StatusCode::TOO_MANY_REQUESTS;
    resp.headers_mut()
        .insert(header::RETRY_AFTER, header::HeaderValue::from_static("60"));
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::{client_ip, RateLimit};
    use axum::body::Body;
    use axum::http::Request;
    use std::net::IpAddr;
    use std::time::{Duration, Instant};

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn allows_up_to_max_then_blocks_within_the_window() {
        let rl = RateLimit::new(3, Duration::from_mins(1));
        let now = Instant::now();
        let who = ip("203.0.113.7");
        assert!(rl.check(who, now)); // 1
        assert!(rl.check(who, now)); // 2
        assert!(rl.check(who, now)); // 3
        assert!(!rl.check(who, now), "4th request in the window is blocked");
    }

    #[test]
    fn the_window_resets_after_it_elapses() {
        let rl = RateLimit::new(1, Duration::from_mins(1));
        let t0 = Instant::now();
        let who = ip("203.0.113.7");
        assert!(rl.check(who, t0));
        assert!(!rl.check(who, t0), "second within window blocked");
        let later = t0 + Duration::from_secs(61);
        assert!(rl.check(who, later), "new window allows again");
    }

    #[test]
    fn limits_are_per_ip() {
        let rl = RateLimit::new(1, Duration::from_mins(1));
        let now = Instant::now();
        assert!(rl.check(ip("198.51.100.1"), now));
        assert!(
            rl.check(ip("198.51.100.2"), now),
            "a different IP has its own budget"
        );
        assert!(
            !rl.check(ip("198.51.100.1"), now),
            "the first IP is now over budget"
        );
    }

    #[test]
    fn disabled_limiter_never_blocks() {
        let rl = RateLimit::disabled();
        let now = Instant::now();
        for _ in 0..1000 {
            assert!(rl.check(ip("203.0.113.9"), now));
        }
    }

    #[test]
    fn client_ip_prefers_the_leftmost_forwarded_for() {
        let req = Request::builder()
            .header("x-forwarded-for", "203.0.113.5, 10.0.0.1")
            .body(Body::empty())
            .unwrap();
        assert_eq!(client_ip(&req), ip("203.0.113.5"));
    }
}
