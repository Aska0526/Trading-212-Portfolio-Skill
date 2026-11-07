//! `t212 report`: a readable terminal overview built from the same read-only
//! tools the MCP server exposes. Nothing here is advice.

use rust_decimal::Decimal;
use std::process::ExitCode;
use t212_core::analytics::{allocation, Allocation};
use t212_core::performance::{performance, Block, Point};
use t212_core::provider::PortfolioProvider;

const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Unicode sparkline, resampled to at most `width` points.
pub fn sparkline(values: &[Decimal], width: usize) -> String {
    if values.is_empty() || width == 0 {
        return String::new();
    }
    let step = values.len().div_ceil(width).max(1);
    let sampled: Vec<Decimal> = values.chunks(step).map(|c| *c.last().unwrap()).collect();
    let (min, max) = sampled
        .iter()
        .fold((sampled[0], sampled[0]), |(lo, hi), v| {
            (lo.min(*v), hi.max(*v))
        });
    let span = max - min;
    sampled
        .iter()
        .map(|v| {
            if span.is_zero() {
                BARS[3]
            } else {
                let idx = ((*v - min) / span * Decimal::from(7)).round();
                BARS[idx.to_string().parse::<usize>().unwrap_or(0).min(7)]
            }
        })
        .collect()
}

fn m(d: Decimal) -> String {
    format!("{:.2}", d.round_dp(2))
}

fn pct(d: Decimal) -> String {
    format!("{:.2}%", (d * Decimal::from(100)).round_dp(2))
}

fn series_line(label: &str, pts: &[Point], ccy: &str) {
    let vals: Vec<Decimal> = pts.iter().map(|p| p.value).collect();
    if let (Some(a), Some(b)) = (pts.first(), pts.last()) {
        println!(
            "  {label:<18} {}  {} → {}  ({} {} → {} {})",
            sparkline(&vals, 40),
            a.date,
            b.date,
            m(a.value),
            ccy,
            m(b.value),
            ccy
        );
    }
}

pub fn print(p: &dyn PortfolioProvider) -> ExitCode {
    let s = match p.account_summary() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("t212: {}: {}", e.code.as_str(), e.message);
            return ExitCode::FAILURE;
        }
    };
    let c = s.currency.as_str();
    println!(
        "\nT212 Portfolio  ·  {}  ·  as of {}\n",
        s.provenance.source, s.provenance.as_of
    );
    println!("  Total value        {} {c}", m(s.total_value.amount));
    println!(
        "  Invested           {} {c}   (cost {} {c})",
        m(s.investments_current_value.amount),
        m(s.investments_total_cost.amount)
    );
    println!(
        "  Unrealised P&L     {} {c}",
        m(s.investments_unrealized_pl.amount)
    );
    println!(
        "  Realised P&L       {} {c}",
        m(s.investments_realized_pl.amount)
    );
    println!(
        "  Free cash          {} {c}",
        m(s.cash_available_to_trade.amount)
    );

    if let Ok(pos) = p.positions() {
        if let Allocation::Available {
            weights,
            hhi,
            excluded_tickers,
            ..
        } = allocation(&pos.positions)
        {
            let mut w = weights.clone();
            w.sort_by_key(|a| std::cmp::Reverse(a.weight));
            println!("\n  Top holdings (share of invested value, cash excluded)");
            for x in w.iter().take(8) {
                let bar = "█".repeat(
                    (x.weight * Decimal::from(30))
                        .round()
                        .to_string()
                        .parse()
                        .unwrap_or(0),
                );
                println!("    {:<16} {:>7}  {bar}", x.ticker, pct(x.weight));
            }
            if w.len() > 8 {
                println!("    … and {} more", w.len() - 8);
            }
            println!(
                "  Concentration (HHI) {}  ·  {} positions",
                hhi.round_dp(3),
                w.len()
            );
            if !excluded_tickers.is_empty() {
                println!(
                    "  Excluded (no market value): {}",
                    excluded_tickers.join(", ")
                );
            }
        }
    }

    eprintln!("\n  Fetching transaction history (rate-limited, may take a moment)…");
    match p.transactions() {
        Ok(t) => {
            let perf = performance(&s, &t);
            println!("\n  Performance");
            match &perf.net_contributions {
                Block::Available(n) => {
                    println!(
                        "  Net deposits       {} {c}  ({} deposit{}, {} withdrawal{})",
                        m(n.total),
                        n.deposits,
                        if n.deposits == 1 { "" } else { "s" },
                        n.withdrawals,
                        if n.withdrawals == 1 { "" } else { "s" }
                    );
                    series_line("Deposits over time", &n.series, c);
                }
                Block::Unavailable { reason } => {
                    println!("  Net deposits       unavailable: {reason}")
                }
            }
            match &perf.total_gain {
                Block::Available(g) => println!(
                    "  Gain vs deposits   {} {c}{}",
                    m(g.amount),
                    g.fraction_of_contributions
                        .map(|f| format!("  ({})", pct(f)))
                        .unwrap_or_default()
                ),
                Block::Unavailable { reason } => {
                    println!("  Gain vs deposits   unavailable: {reason}")
                }
            }
            match &perf.money_weighted_return {
                Block::Available(r) => println!(
                    "  Money-weighted     {} / year since {} ({} days, estimate){}",
                    pct(r.annualised_rate),
                    r.period_start,
                    r.period_days,
                    r.caveat
                        .map(|c| format!("\n                     note: {c}"))
                        .unwrap_or_default()
                ),
                Block::Unavailable { reason } => {
                    println!("  Money-weighted     unavailable: {reason}")
                }
            }
        }
        Err(e) => println!("\n  Performance        unavailable: {}", e.message),
    }
    println!("\n  Descriptive only. Not investment advice.\n");
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn sparkline_scales_and_resamples() {
        let v: Vec<Decimal> = ["1", "2", "3", "4", "5", "6", "7", "8"]
            .iter()
            .map(|s| Decimal::from_str(s).unwrap())
            .collect();
        assert_eq!(sparkline(&v, 8), "▁▂▃▄▅▆▇█");
        assert_eq!(sparkline(&v, 4).chars().count(), 4);
        assert_eq!(sparkline(&[Decimal::ONE, Decimal::ONE], 10), "▄▄");
        assert_eq!(sparkline(&[], 10), "");
    }
}
