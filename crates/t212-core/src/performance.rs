//! Performance over time from the data Trading 212 actually provides.
//!
//! The API exposes no historical valuations or prices, so this module only
//! computes what the inputs support:
//! - `net_contributions`: cumulative DEPOSIT minus WITHDRAW per day.
//! - `total_gain`: current total value minus net contributions.
//! - `money_weighted_return`: annualised XIRR over external cash flows with
//!   the current total value as the terminal flow.
//!
//! Every block reports `available` or `unavailable` with a reason; nothing is
//! filled with zero. Money is summed with `Decimal`. The XIRR root-finder uses
//! f64 for the rate only (an iterative estimate, not a financial sum); the
//! result is rounded to 4 decimal places and labelled as an estimate.

use crate::domain::{AccountSummary, Completeness, History, Transaction};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde::Serialize;
use std::collections::BTreeMap;
use std::str::FromStr;

pub const DEFINITION: &str = "performance_v1: net contributions = cumulative DEPOSIT - WITHDRAW (account currency, by UTC date). total gain = current total value - net contributions. money-weighted return = annualised XIRR of deposits (outflows), withdrawals (inflows) and current total value (terminal inflow), day precision, 365-day year. Fees, interest, dividends and trades are internal to the account and are not external flows. TRANSFER items are counted by the sign of their amount (positive = money into the account); Trading 212 does not document this, so it is an assumption and is reported.";

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Point {
    pub date: String,
    pub value: Decimal,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Block<T: Serialize> {
    Available(T),
    Unavailable { reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Contributions {
    pub total: Decimal,
    pub series: Vec<Point>,
    /// Inflows (deposits and incoming transfers).
    pub deposits: usize,
    /// Outflows (withdrawals and outgoing transfers).
    pub withdrawals: usize,
    /// TRANSFER items counted by sign (an assumption; see definition).
    pub transfers_counted: usize,
}

/// Signed external cash flow for a transaction: positive into the account.
/// DEPOSIT and WITHDRAW use their type; TRANSFER uses the sign of its amount
/// (undocumented by Trading 212, so callers report it as an assumption).
/// Fees, interest and other internal items return `None`.
pub fn external_flow(t: &Transaction) -> Option<Decimal> {
    let a = t.amount.amount;
    match t.kind.as_str() {
        "DEPOSIT" => Some(a.abs()),
        "WITHDRAW" => Some(-a.abs()),
        "TRANSFER" if !a.is_zero() => Some(a),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Gain {
    pub amount: Decimal,
    /// Gain as a fraction of net contributions (None if contributions <= 0).
    pub fraction_of_contributions: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Mwr {
    pub annualised_rate: Decimal,
    pub period_start: String,
    pub period_end: String,
    pub period_days: i64,
    pub method: &'static str,
    pub caveat: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Performance {
    pub definition: &'static str,
    pub currency: String,
    pub as_of: String,
    pub current_total_value: Decimal,
    pub net_contributions: Block<Contributions>,
    pub total_gain: Block<Gain>,
    pub money_weighted_return: Block<Mwr>,
}

/// Days since 1970-01-01 for an RFC 3339 / ISO date prefix `YYYY-MM-DD`.
pub fn days_from_date(s: &str) -> Option<i64> {
    let d = s.get(0..10)?;
    let mut it = d.split('-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let day: i64 = it.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) {
        return None;
    }
    // Days-from-civil (Howard Hinnant).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

fn unavailable<T: Serialize>(reason: impl Into<String>) -> Block<T> {
    Block::Unavailable {
        reason: reason.into(),
    }
}

/// Annualised XIRR. `flows` are (day, amount) with investor outflows negative.
pub fn xirr(flows: &[(i64, f64)]) -> Option<f64> {
    let t0 = flows.iter().map(|f| f.0).min()?;
    let has_neg = flows.iter().any(|f| f.1 < 0.0);
    let has_pos = flows.iter().any(|f| f.1 > 0.0);
    if !has_neg || !has_pos {
        return None;
    }
    let npv = |r: f64| -> f64 {
        flows
            .iter()
            .map(|(d, a)| a / (1.0 + r).powf((d - t0) as f64 / 365.0))
            .sum()
    };
    // Bisection on a wide bracket; robust for the single sign change typical
    // of deposit-then-value flows.
    let (mut lo, mut hi) = (-0.9999_f64, 10.0_f64);
    let (mut flo, fhi) = (npv(lo), npv(hi));
    if !flo.is_finite() || !fhi.is_finite() || flo.signum() == fhi.signum() {
        return None;
    }
    for _ in 0..200 {
        let mid = (lo + hi) / 2.0;
        let fm = npv(mid);
        if fm.abs() < 1e-9 || (hi - lo) < 1e-10 {
            return Some(mid);
        }
        if fm.signum() == flo.signum() {
            lo = mid;
            flo = fm;
        } else {
            hi = mid;
        }
    }
    Some((lo + hi) / 2.0)
}

pub fn performance(summary: &AccountSummary, txs: &History<Transaction>) -> Performance {
    let ccy = summary.currency.clone();
    let as_of = summary.provenance.as_of.clone();
    let current = summary.total_value.amount;

    // ---- net contributions ----
    let contributions: Block<Contributions> = (|| {
        if txs.provenance.completeness != Completeness::Complete {
            return unavailable("transaction history is incomplete (request budget reached or provider data partial)");
        }
        let mut by_day: BTreeMap<String, Decimal> = BTreeMap::new();
        let (mut deposits, mut withdrawals) = (0, 0);
        let mut transfers = 0;
        for t in &txs.items {
            let Some(flow) = external_flow(t) else {
                continue;
            };
            if t.kind == "TRANSFER" {
                transfers += 1;
            }
            if t.amount.currency != ccy {
                return unavailable("a deposit or withdrawal is in a different currency from the account; no FX rates are available");
            }
            let Some(day) = t
                .date_time
                .get(0..10)
                .filter(|d| days_from_date(d).is_some())
            else {
                return unavailable("a transaction has an unparseable date");
            };
            if flow.is_sign_positive() {
                deposits += 1
            } else {
                withdrawals += 1
            }
            *by_day.entry(day.to_owned()).or_default() += flow;
        }
        if deposits == 0 {
            return unavailable("no deposits found in the transaction history");
        }
        let mut running = Decimal::ZERO;
        let series = by_day
            .into_iter()
            .map(|(date, delta)| {
                running += delta;
                Point {
                    date,
                    value: running,
                }
            })
            .collect();
        Block::Available(Contributions {
            total: running,
            series,
            deposits,
            withdrawals,
            transfers_counted: transfers,
        })
    })();

    // ---- total gain ----
    let total_gain = match &contributions {
        Block::Available(c) => {
            let amount = current - c.total;
            let fraction = (c.total > Decimal::ZERO).then(|| (amount / c.total).round_dp(6));
            Block::Available(Gain {
                amount,
                fraction_of_contributions: fraction,
            })
        }
        Block::Unavailable { reason } => unavailable(reason.clone()),
    };

    // ---- money-weighted return ----
    let mwr = match (&contributions, days_from_date(&as_of)) {
        (Block::Unavailable { reason }, _) => unavailable(reason.clone()),
        (_, None) => unavailable("valuation timestamp is unparseable"),
        (Block::Available(c), Some(end)) => {
            let mut flows: Vec<(i64, f64)> = Vec::new();
            let mut prev = Decimal::ZERO;
            for p in &c.series {
                let delta = p.value - prev;
                prev = p.value;
                // Deposit increases contributions -> investor outflow (negative).
                flows.push((
                    days_from_date(&p.date).unwrap(),
                    -delta.to_f64().unwrap_or(0.0),
                ));
            }
            flows.push((end, current.to_f64().unwrap_or(0.0)));
            let start_day = flows.iter().map(|f| f.0).min().unwrap();
            let days = end - start_day;
            match xirr(&flows) {
                Some(r) if days > 0 && r.is_finite() => Block::Available(Mwr {
                    annualised_rate: Decimal::from_str(&format!("{r:.4}")).unwrap_or_default(),
                    period_start: c.series.first().map(|p| p.date.clone()).unwrap_or_default(),
                    period_end: as_of.get(0..10).unwrap_or_default().to_owned(),
                    period_days: days,
                    method: "XIRR (estimate, day precision)",
                    caveat: (days < 365).then_some("period shorter than one year; an annualised rate can exaggerate short-term moves"),
                }),
                _ => unavailable("XIRR has no solution for these cash flows"),
            }
        }
    };

    Performance {
        definition: DEFINITION,
        currency: ccy.as_str().to_owned(),
        as_of,
        current_total_value: current,
        net_contributions: contributions,
        total_gain,
        money_weighted_return: mwr,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{MockProvider, PortfolioProvider};

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    #[test]
    fn date_math() {
        assert_eq!(days_from_date("1970-01-01"), Some(0));
        assert_eq!(days_from_date("2000-03-01T10:00:00Z"), Some(11_017));
        assert_eq!(days_from_date("2025-13-01"), None);
    }

    #[test]
    fn xirr_known_case() {
        // -1000 then +1100 exactly 365 days later -> 10%.
        let r = xirr(&[(0, -1000.0), (365, 1100.0)]).unwrap();
        assert!((r - 0.10).abs() < 1e-6, "{r}");
        assert!(xirr(&[(0, -1.0)]).is_none());
    }

    #[test]
    fn golden_performance_from_fixture() {
        let m = MockProvider::builtin();
        let s = m.account_summary().unwrap();
        let t = m.transactions().unwrap();
        let p = performance(&s, &t);
        // Deposits 800 + 200, withdrawal 100 -> 900; interest ignored.
        let Block::Available(c) = &p.net_contributions else {
            panic!("{p:?}")
        };
        assert_eq!(c.total, d("900.00"));
        assert_eq!(
            c.series.iter().map(|x| x.value).collect::<Vec<_>>(),
            vec![d("800.00"), d("1000.00"), d("900.00")]
        );
        let Block::Available(g) = &p.total_gain else {
            panic!()
        };
        assert_eq!(g.amount, d("150.00")); // 1050 - 900
        assert!(matches!(p.money_weighted_return, Block::Available(_)));
    }

    #[test]
    fn transfers_counted_by_sign_and_incomplete_history_unavailable() {
        let m = MockProvider::builtin();
        let s = m.account_summary().unwrap();
        // Turn the first 800 deposit into an incoming 800 transfer: same total.
        let mut t = m.transactions().unwrap();
        t.items[0].kind = "TRANSFER".into();
        let p = performance(&s, &t);
        let Block::Available(c) = &p.net_contributions else {
            panic!("{p:?}")
        };
        assert_eq!(c.total, d("900.00"));
        assert_eq!(c.transfers_counted, 1);
        // An outgoing (negative) transfer reduces contributions.
        t.items[0].amount.amount = d("-800.00");
        let Block::Available(c) = performance(&s, &t).net_contributions else {
            panic!()
        };
        assert_eq!(c.total, d("-700.00"));
        let mut t = m.transactions().unwrap();
        t.provenance.completeness = Completeness::Partial;
        assert!(matches!(
            performance(&s, &t).money_weighted_return,
            Block::Unavailable { .. }
        ));
    }
}
