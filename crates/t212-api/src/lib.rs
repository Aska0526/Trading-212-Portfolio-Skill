//! Read-only Trading 212 API adapter.
//!
//! Safety properties:
//! - Only the two fixed hosts in [`Environment`] are ever contacted, over HTTPS.
//! - Only the exact GET routes in [`ROUTES`] can be requested; there is no
//!   generic URL, method, header or body input anywhere in this crate.
//! - Redirects are never followed; responses are size-bounded.
//! - Credentials live in memory only for the request and never appear in
//!   errors or `Debug` output.
//!
//! Endpoint schemas were taken from the official OpenAPI bundle
//! (https://docs.trading212.com/_bundle/api.json), reviewed 2026-11-07.

mod client;
mod normalize;
mod transport;

pub use client::{Credentials, T212Provider};
pub use transport::{RawResponse, Transport, UreqTransport};

/// Trading 212 environment. Hosts are fixed; nothing else can be targeted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    Demo,
    Live,
}

impl Environment {
    pub fn host(self) -> &'static str {
        match self {
            Environment::Demo => "demo.trading212.com",
            Environment::Live => "live.trading212.com",
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Environment::Demo => "demo",
            Environment::Live => "live",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "demo" => Some(Environment::Demo),
            "live" => Some(Environment::Live),
            _ => None,
        }
    }
}

pub const ACCOUNT_SUMMARY_PATH: &str = "/api/v0/equity/account/summary";
pub const POSITIONS_PATH: &str = "/api/v0/equity/positions";
pub const TRANSACTIONS_PATH: &str = "/api/v0/equity/history/transactions";
pub const DIVIDENDS_PATH: &str = "/api/v0/equity/history/dividends";
pub const ORDERS_HISTORY_PATH: &str = "/api/v0/equity/history/orders";

/// The complete set of routes this crate may request (all GET).
pub const ROUTES: &[&str] = &[
    ACCOUNT_SUMMARY_PATH,
    POSITIONS_PATH,
    TRANSACTIONS_PATH,
    DIVIDENDS_PATH,
    ORDERS_HISTORY_PATH,
];

/// Routes that use cursor pagination via `nextPagePath`.
pub const PAGINATED_ROUTES: &[&str] = &[TRANSACTIONS_PATH, DIVIDENDS_PATH, ORDERS_HISTORY_PATH];

/// Query parameters this crate will send or follow (documented for the
/// paginated routes). Anything else in a continuation path is rejected.
pub const ALLOWED_QUERY_KEYS: &[&str] = &["limit", "cursor", "time", "ticker"];

/// Validate a request target (`path` or `path?query`) against the allowlist.
/// Returns the base route on success.
pub fn validate_target(target: &str) -> Option<&'static str> {
    if target.len() > 512 || target.contains('#') || target.contains("..") || target.contains("//")
    {
        return None;
    }
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (target, None),
    };
    let route = ROUTES.iter().copied().find(|r| *r == path)?;
    if let Some(q) = query {
        if !PAGINATED_ROUTES.contains(&route) {
            return None;
        }
        for pair in q.split('&') {
            let (k, v) = pair.split_once('=')?;
            let ok_val = !v.is_empty()
                && v.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.:%+".contains(&b));
            if !ALLOWED_QUERY_KEYS.contains(&k) || !ok_val {
                return None;
            }
        }
    }
    Some(route)
}
