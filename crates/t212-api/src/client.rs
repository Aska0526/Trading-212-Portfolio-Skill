use crate::normalize;
use crate::transport::{RawResponse, Transport};
use crate::{
    validate_target, Environment, ACCOUNT_SUMMARY_PATH, DIVIDENDS_PATH, ORDERS_HISTORY_PATH,
    POSITIONS_PATH, TRANSACTIONS_PATH,
};
use base64::Engine;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashSet;
use std::time::{Duration, Instant};
use t212_core::domain::{
    AccountSummary, Completeness, Dividend, Fill, History, Positions, Provenance, Transaction,
};
use t212_core::error::{ErrorCode, T212Error};
use t212_core::provider::PortfolioProvider;

/// API key pair. `Debug` never prints the values.
#[derive(Clone)]
pub struct Credentials {
    pub key: String,
    pub secret: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Credentials([REDACTED])")
    }
}

/// Maximum pages fetched per history read (50 items each). Beyond this the
/// result is returned and marked partial rather than hammering the API.
pub const MAX_PAGES: usize = 40;
/// History endpoints allow 20 requests/minute; pace pages to stay under it.
pub const HISTORY_PACE: Duration = Duration::from_millis(3_100);
/// Longest rate-limit reset we will wait for inside one tool call.
pub const MAX_RATE_WAIT_SECS: u64 = 20;
/// In-process cache lifetime for history reads (not persisted).
pub const HISTORY_TTL: Duration = Duration::from_secs(300);

type Cached<T> = RefCell<Option<(Instant, History<T>)>>;

pub struct T212Provider<T: Transport> {
    transport: T,
    env: Environment,
    creds: Credentials,
    pace: Duration,
    tx_cache: Cached<Transaction>,
    div_cache: Cached<Dividend>,
    fill_cache: Cached<Fill>,
}

pub fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

impl<T: Transport> T212Provider<T> {
    pub fn new(transport: T, env: Environment, creds: Credentials) -> Self {
        Self {
            transport,
            env,
            creds,
            pace: HISTORY_PACE,
            tx_cache: RefCell::new(None),
            div_cache: RefCell::new(None),
            fill_cache: RefCell::new(None),
        }
    }

    /// Override page pacing (tests use zero).
    pub fn with_pace(mut self, pace: Duration) -> Self {
        self.pace = pace;
        self
    }

    pub fn source(&self) -> String {
        format!("trading212-{}", self.env.as_str())
    }

    fn raw_get(&self, target: &str) -> Result<RawResponse, T212Error> {
        if validate_target(target).is_none() {
            return Err(T212Error::new(
                ErrorCode::Internal,
                "route is not allowlisted",
            ));
        }
        let token = base64::engine::general_purpose::STANDARD
            .encode(format!("{}:{}", self.creds.key, self.creds.secret));
        let auth = format!("Basic {token}");
        self.transport.get(self.env, target, &auth)
    }

    fn get_json(&self, target: &str) -> Result<Value, T212Error> {
        let resp = self.raw_get(target)?;
        check_status(&resp)?;
        parse(&resp)
    }

    /// Follow `nextPagePath` from `route` with validation, cycle detection,
    /// pacing and a page budget. Returns raw items and whether the end was
    /// reached.
    fn paginate(&self, route: &'static str) -> Result<(Vec<Value>, bool), T212Error> {
        let mut target = format!("{route}?limit=50");
        let mut seen = HashSet::new();
        let mut items = Vec::new();
        for page in 0..MAX_PAGES {
            if !seen.insert(target.clone()) {
                // Cycle: stop safely, keep what we have, report partial.
                return Ok((items, false));
            }
            if page > 0 && !self.pace.is_zero() {
                std::thread::sleep(self.pace);
            }
            let mut resp = self.raw_get(&target)?;
            if resp.status == 429 {
                match resp.retry_after_secs {
                    Some(s) if s <= MAX_RATE_WAIT_SECS && !self.pace.is_zero() => {
                        std::thread::sleep(Duration::from_secs(s + 1));
                        resp = self.raw_get(&target)?;
                    }
                    _ if !items.is_empty() => return Ok((items, false)),
                    _ => {}
                }
            }
            if resp.status == 429 && !items.is_empty() {
                return Ok((items, false));
            }
            check_status(&resp)?;
            let v = parse(&resp)?;
            let page_items = v.get("items").and_then(Value::as_array).ok_or_else(|| {
                T212Error::new(
                    ErrorCode::CompatibilityError,
                    "history page has no items array",
                )
            })?;
            items.extend(page_items.iter().cloned());
            match v.get("nextPagePath") {
                None | Some(Value::Null) => return Ok((items, true)),
                Some(Value::String(next)) => match validate_target(next) {
                    Some(r) if r == route => target = next.clone(),
                    _ => return Err(T212Error::new(
                        ErrorCode::CompatibilityError,
                        "Trading 212 returned a continuation path outside the allowlist; stopped",
                    )),
                },
                Some(_) => {
                    return Err(T212Error::new(
                        ErrorCode::CompatibilityError,
                        "malformed continuation path",
                    ))
                }
            }
        }
        Ok((items, false))
    }

    fn history<I: Clone>(
        &self,
        cache: &Cached<I>,
        route: &'static str,
        map: fn(&Value) -> Option<I>,
    ) -> Result<History<I>, T212Error> {
        if let Some((at, h)) = cache.borrow().as_ref() {
            if at.elapsed() < HISTORY_TTL {
                return Ok(h.clone());
            }
        }
        let (mut raw, reached_end) = self.paginate(route)?;
        if route == ORDERS_HISTORY_PATH {
            raw.retain(|v| !normalize::is_unfilled_order(v));
        }
        let mut skipped = false;
        let items: Vec<I> = raw
            .iter()
            .filter_map(|v| {
                let r = map(v);
                skipped |= r.is_none();
                r
            })
            .collect();
        let complete = reached_end && !skipped;
        let h = History {
            items,
            provenance: Provenance {
                source: self.source(),
                as_of: now_rfc3339(),
                completeness: if complete {
                    Completeness::Complete
                } else {
                    Completeness::Partial
                },
                stale: false,
            },
        };
        *cache.borrow_mut() = Some((Instant::now(), h.clone()));
        Ok(h)
    }
}

fn parse(r: &RawResponse) -> Result<Value, T212Error> {
    serde_json::from_slice(&r.body)
        .map_err(|_| T212Error::new(ErrorCode::CompatibilityError, "response was not valid JSON"))
}

fn check_status(r: &RawResponse) -> Result<(), T212Error> {
    match r.status {
        200 => Ok(()),
        401 | 403 => Err(T212Error::new(
            ErrorCode::AuthenticationFailed,
            "Trading 212 rejected the credentials or the key lacks the needed read permission. \
             Re-run `t212 setup` in a terminal (do not paste credentials into chat).",
        )),
        429 => Err(T212Error::new(
            ErrorCode::RateLimited,
            match r.retry_after_secs {
                Some(s) => format!("rate limited by Trading 212; retry in about {s} seconds"),
                None => "rate limited by Trading 212; retry shortly".to_string(),
            },
        )),
        408 | 500..=599 => Err(T212Error::new(
            ErrorCode::ProviderUnavailable,
            "Trading 212 is temporarily unavailable",
        )),
        _ => Err(T212Error::new(
            ErrorCode::CompatibilityError,
            "unexpected response status from Trading 212",
        )),
    }
}

impl<T: Transport> PortfolioProvider for T212Provider<T> {
    fn account_summary(&self) -> Result<AccountSummary, T212Error> {
        let v = self.get_json(ACCOUNT_SUMMARY_PATH)?;
        normalize::account_summary(&v, self.source(), now_rfc3339())
    }

    fn positions(&self) -> Result<Positions, T212Error> {
        let v = self.get_json(POSITIONS_PATH)?;
        normalize::positions(&v, self.source(), now_rfc3339())
    }

    fn transactions(&self) -> Result<History<Transaction>, T212Error> {
        self.history(&self.tx_cache, TRANSACTIONS_PATH, normalize::transaction)
    }

    fn dividends(&self) -> Result<History<Dividend>, T212Error> {
        self.history(&self.div_cache, DIVIDENDS_PATH, normalize::dividend)
    }

    fn fills(&self) -> Result<History<Fill>, T212Error> {
        self.history(&self.fill_cache, ORDERS_HISTORY_PATH, normalize::fill)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ROUTES;
    use std::collections::HashMap;

    /// Fake transport: maps exact request targets to (status, body).
    struct Fake {
        routes: HashMap<String, (u16, String)>,
        seen: RefCell<Vec<(String, String)>>,
    }
    impl Fake {
        fn new(pairs: &[(&str, u16, &str)]) -> Self {
            Self {
                routes: pairs
                    .iter()
                    .map(|(t, s, b)| (t.to_string(), (*s, b.to_string())))
                    .collect(),
                seen: RefCell::new(vec![]),
            }
        }
    }
    impl Transport for Fake {
        fn get(&self, _: Environment, path: &str, auth: &str) -> Result<RawResponse, T212Error> {
            self.seen
                .borrow_mut()
                .push((path.to_string(), auth.to_string()));
            let (status, body) = self.routes.get(path).cloned().unwrap_or((404, "{}".into()));
            Ok(RawResponse {
                status,
                body: body.into_bytes(),
                retry_after_secs: Some(3),
            })
        }
    }
    fn provider(pairs: &[(&str, u16, &str)]) -> T212Provider<Fake> {
        T212Provider::new(
            Fake::new(pairs),
            Environment::Demo,
            Credentials {
                key: "KEYCANARY".into(),
                secret: "SECRETCANARY".into(),
            },
        )
        .with_pace(Duration::ZERO)
    }

    const SUMMARY: &str = r#"{"cash":{"availableToTrade":50.5,"inPies":0,"reservedForOrders":0},
      "currency":"GBP","id":123456,"investments":{"currentValue":1000,"realizedProfitLoss":10,
      "totalCost":900,"unrealizedProfitLoss":100},"totalValue":1050.5}"#;
    const POSITIONS: &str = r#"[{"averagePricePaid":10,"createdAt":"2024-01-01T00:00:00Z",
      "currentPrice":12.5,"instrument":{"currency":"USD","isin":"X","name":"Alpha","ticker":"AAA_US_EQ"},
      "quantity":10.25,"quantityAvailableForTrading":10.25,"quantityInPies":0,
      "walletImpact":{"currency":"GBP","currentValue":100.1,"fxImpact":null,"totalCost":90,"unrealizedProfitLoss":10.1}}]"#;

    #[test]
    fn normalizes_summary_without_account_id() {
        let p = provider(&[(ACCOUNT_SUMMARY_PATH, 200, SUMMARY)]);
        let s = p.account_summary().unwrap();
        assert_eq!(s.total_value.amount.to_string(), "1050.5");
        assert_eq!(s.currency.as_str(), "GBP");
        assert!(!serde_json::to_string(&s).unwrap().contains("123456"));
        let seen = p.transport.seen.borrow();
        assert_eq!(seen[0].0, ACCOUNT_SUMMARY_PATH);
        assert!(seen[0].1.starts_with("Basic "));
    }

    #[test]
    fn normalizes_positions_decimal_safe() {
        let p = provider(&[(POSITIONS_PATH, 200, POSITIONS)])
            .positions()
            .unwrap();
        let pos = &p.positions[0];
        assert_eq!(pos.quantity.to_string(), "10.25");
        assert_eq!(
            pos.market_value.as_ref().unwrap().amount.to_string(),
            "100.1"
        );
    }

    #[test]
    fn auth_failure_is_typed_and_leaks_nothing() {
        let e = provider(&[(ACCOUNT_SUMMARY_PATH, 401, "{}")])
            .account_summary()
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::AuthenticationFailed);
        assert!(!format!("{e} {e:?}").contains("CANARY"));
        assert!(!format!(
            "{:?}",
            Credentials {
                key: "KEYCANARY".into(),
                secret: "S".into()
            }
        )
        .contains("CANARY"));
    }

    #[test]
    fn rate_limit_reports_reset() {
        let e = provider(&[(POSITIONS_PATH, 429, "{}")])
            .positions()
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::RateLimited);
        assert!(e.message.contains('3'));
    }

    #[test]
    fn schema_drift_fails_closed_not_zero() {
        let e = provider(&[(ACCOUNT_SUMMARY_PATH, 200, r#"{"currency":"GBP"}"#)])
            .account_summary()
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::CompatibilityError);
    }

    #[test]
    fn non_allowlisted_targets_refused_before_any_request() {
        let p = provider(&[]);
        for bad in [
            "/api/v0/equity/orders",
            "/api/v0/equity/positions?x=1",
            "/api/v0/equity/history/transactions?cursor=1&evil=2",
            "/api/v0/equity/history/transactions?cursor=../x",
            "//evil.example.com/api/v0/equity/positions",
            "/api/v0/equity/history/orders?cursor=1#frag",
        ] {
            assert!(p.get_json(bad).is_err(), "{bad}");
        }
        assert!(p.transport.seen.borrow().is_empty());
    }

    #[test]
    fn route_list_has_no_mutation_or_order_placement_paths() {
        for r in ROUTES {
            assert!(!r.contains("/pies") && !r.contains("/exports"));
            assert!(*r != "/api/v0/equity/orders" && !r.starts_with("/api/v0/equity/orders/"));
        }
    }

    const TX1: &str = r#"{"items":[{"amount":500,"currency":"GBP","dateTime":"2025-01-01T09:00:00Z","reference":"r1","type":"DEPOSIT"}],
      "nextPagePath":"/api/v0/equity/history/transactions?limit=50&cursor=abc"}"#;
    const TX2: &str = r#"{"items":[{"amount":-50.25,"currency":"GBP","dateTime":"2025-02-01T09:00:00Z","reference":"r2","type":"WITHDRAW"}],
      "nextPagePath":null}"#;

    #[test]
    fn follows_validated_pagination_to_the_end() {
        let p = provider(&[
            ("/api/v0/equity/history/transactions?limit=50", 200, TX1),
            (
                "/api/v0/equity/history/transactions?limit=50&cursor=abc",
                200,
                TX2,
            ),
        ]);
        let h = p.transactions().unwrap();
        assert_eq!(h.items.len(), 2);
        assert_eq!(h.items[1].amount.amount.to_string(), "-50.25");
        assert_eq!(h.provenance.completeness, Completeness::Complete);
        // Cached: a second call makes no new requests.
        let before = p.transport.seen.borrow().len();
        p.transactions().unwrap();
        assert_eq!(p.transport.seen.borrow().len(), before);
    }

    #[test]
    fn cyclic_pagination_stops_and_is_partial() {
        let looped = r#"{"items":[{"amount":1,"currency":"GBP","dateTime":"2025-01-01T00:00:00Z","type":"DEPOSIT"}],
          "nextPagePath":"/api/v0/equity/history/transactions?limit=50"}"#;
        let p = provider(&[("/api/v0/equity/history/transactions?limit=50", 200, looped)]);
        let h = p.transactions().unwrap();
        assert_eq!(h.provenance.completeness, Completeness::Partial);
        assert_eq!(p.transport.seen.borrow().len(), 1);
    }

    #[test]
    fn off_allowlist_continuation_is_rejected() {
        let evil = r#"{"items":[],"nextPagePath":"/api/v0/equity/orders?cursor=1"}"#;
        let p = provider(&[("/api/v0/equity/history/transactions?limit=50", 200, evil)]);
        assert_eq!(
            p.transactions().unwrap_err().code,
            ErrorCode::CompatibilityError
        );
        assert_eq!(p.transport.seen.borrow().len(), 1);
    }

    #[test]
    fn dividends_and_fills_normalize() {
        let div = r#"{"items":[{"amount":4.1,"currency":"GBP","paidOn":"2025-06-15T00:00:00Z","ticker":"AAA_US_EQ","type":"ORDINARY"}],"nextPagePath":null}"#;
        let ord = r#"{"items":[
          {"order":{"ticker":"AAA_US_EQ","side":"BUY","status":"FILLED"},
           "fill":{"filledAt":"2025-01-02T14:30:00Z","price":60.5,"quantity":10,"type":"TRADE",
                   "walletImpact":{"currency":"GBP","netValue":-500,"realisedProfitLoss":0,"fxRate":1}}},
          {"order":{"ticker":"BBB_US_EQ","side":"BUY","status":"CANCELLED"}}],"nextPagePath":null}"#;
        let p = provider(&[
            ("/api/v0/equity/history/dividends?limit=50", 200, div),
            ("/api/v0/equity/history/orders?limit=50", 200, ord),
        ]);
        let d = p.dividends().unwrap();
        assert_eq!(d.items[0].amount.amount.to_string(), "4.1");
        let f = p.fills().unwrap();
        assert_eq!(f.items.len(), 1, "unfilled orders are not fills");
        assert_eq!(
            f.items[0].net_value.as_ref().unwrap().amount.to_string(),
            "-500"
        );
        assert_eq!(f.provenance.completeness, Completeness::Complete);
    }
}
