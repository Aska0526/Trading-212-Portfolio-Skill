//! Raw provider JSON -> normalized contract. Fails closed (CompatibilityError)
//! when a required field is missing or has the wrong type. A null/absent
//! optional value stays `None`; it is never converted to zero.

use rust_decimal::Decimal;
use serde_json::Value;
use std::str::FromStr;
use t212_core::domain::{
    AccountSummary, Completeness, Currency, Dividend, Fill, Money, Position, Positions, Provenance,
    Transaction,
};
use t212_core::error::{ErrorCode, T212Error};

fn drift() -> T212Error {
    T212Error::new(
        ErrorCode::CompatibilityError,
        "Trading 212 response did not match the expected schema",
    )
}

fn dec(v: &Value) -> Result<Decimal, T212Error> {
    let n = v.as_number().ok_or_else(drift)?;
    let s = n.to_string();
    Decimal::from_str(&s)
        .or_else(|_| Decimal::from_scientific(&s))
        .map_err(|_| drift())
}

fn field<'a>(v: &'a Value, k: &str) -> Result<&'a Value, T212Error> {
    v.get(k).filter(|x| !x.is_null()).ok_or_else(drift)
}

fn currency(v: &Value) -> Result<Currency, T212Error> {
    Currency::new(v.as_str().ok_or_else(drift)?).ok_or_else(drift)
}

fn money(v: &Value, k: &str, c: &Currency) -> Result<Money, T212Error> {
    Ok(Money {
        amount: dec(field(v, k)?)?,
        currency: c.clone(),
    })
}

pub fn account_summary(
    v: &Value,
    source: String,
    as_of: String,
) -> Result<AccountSummary, T212Error> {
    let c = currency(field(v, "currency")?)?;
    let cash = field(v, "cash")?;
    let inv = field(v, "investments")?;
    Ok(AccountSummary {
        total_value: money(v, "totalValue", &c)?,
        cash_available_to_trade: money(cash, "availableToTrade", &c)?,
        cash_in_pies: money(cash, "inPies", &c)?,
        cash_reserved_for_orders: money(cash, "reservedForOrders", &c)?,
        investments_current_value: money(inv, "currentValue", &c)?,
        investments_total_cost: money(inv, "totalCost", &c)?,
        investments_unrealized_pl: money(inv, "unrealizedProfitLoss", &c)?,
        investments_realized_pl: money(inv, "realizedProfitLoss", &c)?,
        currency: c,
        provenance: Provenance {
            source,
            as_of,
            completeness: Completeness::Complete,
            stale: false,
        },
    })
}

pub fn positions(v: &Value, source: String, as_of: String) -> Result<Positions, T212Error> {
    let arr = v.as_array().ok_or_else(drift)?;
    let mut out = Vec::with_capacity(arr.len());
    let mut partial = false;
    for p in arr {
        let instrument = field(p, "instrument")?;
        let ticker = field(instrument, "ticker")?
            .as_str()
            .ok_or_else(drift)?
            .to_owned();
        let name = instrument
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let quantity = dec(field(p, "quantity")?)?;
        // Wallet impact is in the account currency. If absent/invalid, keep
        // the position but mark values unavailable and the snapshot partial.
        let wallet = p.get("walletImpact").filter(|w| w.is_object());
        let parsed = wallet.and_then(|w| {
            let c = currency(w.get("currency")?).ok()?;
            Some((
                money(w, "currentValue", &c).ok()?,
                money(w, "totalCost", &c).ok()?,
                money(w, "unrealizedProfitLoss", &c).ok()?,
            ))
        });
        let (market_value, total_cost, unrealized_pl) = match parsed {
            Some((a, b, c)) => (Some(a), Some(b), Some(c)),
            None => {
                partial = true;
                (None, None, None)
            }
        };
        out.push(Position {
            ticker,
            name,
            quantity,
            market_value,
            total_cost,
            unrealized_pl,
        });
    }
    out.sort_by(|a, b| a.ticker.cmp(&b.ticker));
    Ok(Positions {
        positions: out,
        provenance: Provenance {
            source,
            as_of,
            completeness: if partial {
                Completeness::Partial
            } else {
                Completeness::Complete
            },
            stale: false,
        },
    })
}

// ---- history items: `None` means the item did not match the documented
// schema; the caller drops it and marks the whole result partial. ----

fn opt_str(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_owned)
}

fn opt_money(v: &Value, k: &str, c: &Currency) -> Option<Money> {
    money(v, k, c).ok()
}

pub fn transaction(v: &Value) -> Option<Transaction> {
    let c = currency(v.get("currency")?).ok()?;
    Some(Transaction {
        date_time: opt_str(v, "dateTime")?,
        kind: opt_str(v, "type")?,
        amount: opt_money(v, "amount", &c)?,
    })
}

pub fn dividend(v: &Value) -> Option<Dividend> {
    let c = currency(v.get("currency")?).ok()?;
    let ticker =
        opt_str(v, "ticker").or_else(|| v.get("instrument").and_then(|i| opt_str(i, "ticker")))?;
    Some(Dividend {
        paid_on: opt_str(v, "paidOn")?,
        ticker,
        kind: opt_str(v, "type").unwrap_or_else(|| "UNKNOWN".into()),
        amount: opt_money(v, "amount", &c)?,
    })
}

/// Unfilled orders are removed before this runs (see `is_unfilled_order`).
pub fn fill(v: &Value) -> Option<Fill> {
    let order = v.get("order")?;
    let fill = v.get("fill")?;
    let ticker = opt_str(order, "ticker")
        .or_else(|| order.get("instrument").and_then(|i| opt_str(i, "ticker")))?;
    let wallet = fill.get("walletImpact").filter(|w| w.is_object());
    let wc = wallet.and_then(|w| currency(w.get("currency")?).ok());
    Some(Fill {
        filled_at: opt_str(fill, "filledAt")?,
        ticker,
        side: opt_str(order, "side"),
        quantity: dec(fill.get("quantity")?).ok()?,
        price: fill.get("price").and_then(|p| dec(p).ok()),
        net_value: wallet
            .zip(wc.as_ref())
            .and_then(|(w, c)| opt_money(w, "netValue", c)),
        realised_pl: wallet
            .zip(wc.as_ref())
            .and_then(|(w, c)| opt_money(w, "realisedProfitLoss", c)),
    })
}

/// Orders with no fill object are legitimately not fills.
pub fn is_unfilled_order(v: &Value) -> bool {
    v.get("order").is_some() && v.get("fill").is_none_or(Value::is_null)
}
