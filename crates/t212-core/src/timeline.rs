//! Cumulative series built only from Trading 212 history (no prices).
//!
//! Each series is a list of (date, cumulative value) points at the dates where
//! something happened, in the account currency. Series are independent: a
//! problem with one (partial history, mixed currency) makes only that one
//! unavailable.

use crate::domain::{Completeness, Currency, Dividend, Fill, History, Transaction};
use crate::performance::{external_flow, Block, Point};
use rust_decimal::Decimal;
use serde::Serialize;
use std::collections::BTreeMap;

pub const DEFINITION: &str = "timeline_v1: cumulative sums by UTC date in the account currency. net_deposits = DEPOSIT - WITHDRAW + TRANSFER by sign (assumed: positive = into the account). dividends = paid dividend amounts. realised_pl = provider-reported realised P&L on executed fills. No historical prices are used; these are not portfolio values.";

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Series {
    pub points: Vec<Point>,
    pub total: Decimal,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Timeline {
    pub definition: &'static str,
    pub currency: String,
    pub net_deposits: Block<Series>,
    pub dividends: Block<Series>,
    pub realised_pl: Block<Series>,
}

/// Cumulative series from (date, amount) items. `None` amount means the item
/// carries no value for this series and is skipped.
fn cumulate<'a>(
    complete: bool,
    ccy: &Currency,
    items: impl Iterator<Item = (&'a str, Option<(&'a Currency, Decimal)>)>,
) -> Block<Series> {
    if !complete {
        return Block::Unavailable {
            reason: "history is incomplete (request budget reached or provider data partial)"
                .into(),
        };
    }
    let mut by_day: BTreeMap<String, Decimal> = BTreeMap::new();
    for (date, amount) in items {
        let Some((c, a)) = amount else { continue };
        if c != ccy {
            return Block::Unavailable {
                reason:
                    "an item is in a different currency from the account; no FX rates are available"
                        .into(),
            };
        }
        let Some(day) = date.get(0..10) else {
            return Block::Unavailable {
                reason: "an item has an unparseable date".into(),
            };
        };
        *by_day.entry(day.to_owned()).or_default() += a;
    }
    let mut total = Decimal::ZERO;
    let points = by_day
        .into_iter()
        .map(|(date, d)| {
            total += d;
            Point { date, value: total }
        })
        .collect();
    Block::Available(Series { points, total })
}

pub fn timeline(
    ccy: &Currency,
    txs: &History<Transaction>,
    divs: &History<Dividend>,
    fills: &History<Fill>,
) -> Timeline {
    let done = |c: Completeness| c == Completeness::Complete;
    let net_deposits = cumulate(
        done(txs.provenance.completeness),
        ccy,
        txs.items.iter().map(|t| {
            (
                t.date_time.as_str(),
                external_flow(t).map(|a| (&t.amount.currency, a)),
            )
        }),
    );
    let dividends = cumulate(
        done(divs.provenance.completeness),
        ccy,
        divs.items.iter().map(|d| {
            (
                d.paid_on.as_str(),
                Some((&d.amount.currency, d.amount.amount)),
            )
        }),
    );
    let realised_pl = cumulate(
        done(fills.provenance.completeness),
        ccy,
        fills.items.iter().map(|f| {
            (
                f.filled_at.as_str(),
                f.realised_pl.as_ref().map(|m| (&m.currency, m.amount)),
            )
        }),
    );
    Timeline {
        definition: DEFINITION,
        currency: ccy.as_str().to_owned(),
        net_deposits,
        dividends,
        realised_pl,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{MockProvider, PortfolioProvider};
    use std::str::FromStr;

    #[test]
    fn golden_timeline_from_fixture() {
        let m = MockProvider::builtin();
        let ccy = m.account_summary().unwrap().currency;
        let t = timeline(
            &ccy,
            &m.transactions().unwrap(),
            &m.dividends().unwrap(),
            &m.fills().unwrap(),
        );
        let Block::Available(nd) = &t.net_deposits else {
            panic!("{t:?}")
        };
        let vals: Vec<String> = nd.points.iter().map(|p| p.value.to_string()).collect();
        assert_eq!(vals, ["800.00", "1000.00", "900.00"]);
        let Block::Available(dv) = &t.dividends else {
            panic!()
        };
        assert_eq!(dv.total, Decimal::from_str("8.40").unwrap());
        // The fixture fill has no realised P&L: available, empty, total 0
        // (no realised events is a fact, not missing data).
        let Block::Available(rp) = &t.realised_pl else {
            panic!()
        };
        assert!(rp.points.is_empty());
    }

    #[test]
    fn partial_history_makes_only_that_series_unavailable() {
        let m = MockProvider::builtin();
        let ccy = m.account_summary().unwrap().currency;
        let mut d = m.dividends().unwrap();
        d.provenance.completeness = Completeness::Partial;
        let t = timeline(&ccy, &m.transactions().unwrap(), &d, &m.fills().unwrap());
        assert!(matches!(t.dividends, Block::Unavailable { .. }));
        assert!(matches!(t.net_deposits, Block::Available(_)));
    }
}
