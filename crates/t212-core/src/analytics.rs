//! Pure, deterministic analytics over normalized models.
//!
//! Every result carries its definition, base currency and exclusions. Missing
//! or incompatible inputs yield `Unavailable`, never a silent zero.

use crate::domain::{Currency, Position};
use rust_decimal::Decimal;
use serde::Serialize;

pub const ALLOCATION_DEFINITION: &str = "allocation_v1: weight = position market value / sum of included position market values (cash excluded). Positions without a verified market value are excluded and listed.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Weight {
    pub ticker: String,
    pub weight: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Allocation {
    Available {
        definition: &'static str,
        currency: Currency,
        denominator: Decimal,
        cash_included: bool,
        weights: Vec<Weight>,
        /// Sum of squared weights over included positions.
        hhi: Decimal,
        excluded_tickers: Vec<String>,
    },
    Unavailable {
        definition: &'static str,
        reason: String,
    },
}

/// Weights are rounded to 6 decimal places (midpoint away from zero).
pub fn allocation(positions: &[Position]) -> Allocation {
    let unavailable = |reason: &str| Allocation::Unavailable {
        definition: ALLOCATION_DEFINITION,
        reason: reason.to_owned(),
    };
    let mut included = Vec::new();
    let mut excluded = Vec::new();
    for pos in positions {
        match &pos.market_value {
            Some(m) => included.push((pos.ticker.clone(), m)),
            None => excluded.push(pos.ticker.clone()),
        }
    }
    let Some((_, first)) = included.first() else {
        return unavailable("no positions with a verified market value");
    };
    let currency = first.currency.clone();
    if included.iter().any(|(_, m)| m.currency != currency) {
        return unavailable("mixed currencies and no verified FX rates");
    }
    let denominator: Decimal = included.iter().map(|(_, m)| m.amount).sum();
    if denominator <= Decimal::ZERO {
        return unavailable("non-positive total value");
    }
    included.sort_by(|a, b| a.0.cmp(&b.0));
    excluded.sort();
    let fractions: Vec<(String, Decimal)> = included
        .iter()
        .map(|(t, m)| (t.clone(), m.amount / denominator))
        .collect();
    let hhi = fractions
        .iter()
        .map(|(_, w)| *w * *w)
        .sum::<Decimal>()
        .round_dp(6);
    let weights = fractions
        .into_iter()
        .map(|(ticker, w)| Weight {
            ticker,
            weight: w.round_dp(6),
        })
        .collect();
    Allocation::Available {
        definition: ALLOCATION_DEFINITION,
        currency,
        denominator,
        cash_included: false,
        weights,
        hhi,
        excluded_tickers: excluded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{MockProvider, PortfolioProvider};
    use std::str::FromStr;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    #[test]
    fn golden_allocation_from_fixture() {
        // Hand-computed: values 600/300/100 of 1000 -> 0.6/0.3/0.1, HHI 0.46.
        let p = MockProvider::builtin().positions().unwrap().positions;
        match allocation(&p) {
            Allocation::Available {
                weights,
                hhi,
                excluded_tickers,
                ..
            } => {
                assert_eq!(weights[0].weight, d("0.6"));
                assert_eq!(weights[1].weight, d("0.3"));
                assert_eq!(weights[2].weight, d("0.1"));
                assert_eq!(hhi, d("0.46"));
                assert_eq!(excluded_tickers, vec!["NOVAL".to_string()]);
            }
            other => panic!("expected available, got {other:?}"),
        }
    }

    #[test]
    fn mixed_currency_is_unavailable() {
        let mut p = MockProvider::builtin().positions().unwrap().positions;
        p[0].market_value.as_mut().unwrap().currency = Currency::new("EUR").unwrap();
        assert!(matches!(allocation(&p), Allocation::Unavailable { .. }));
    }

    #[test]
    fn no_values_is_unavailable_not_zero() {
        let mut p = MockProvider::builtin().positions().unwrap().positions;
        for pos in &mut p {
            pos.market_value = None;
        }
        assert!(matches!(allocation(&p), Allocation::Unavailable { .. }));
    }
}
