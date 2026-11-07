//! Versioned normalized domain models. Provider DTOs never appear here.
//! Account identifiers are deliberately not part of the contract.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// ISO-4217-style currency code, kept as an opaque validated string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Currency(String);

impl Currency {
    pub fn new(code: &str) -> Option<Self> {
        let ok = code.len() == 3 && code.bytes().all(|b| b.is_ascii_uppercase());
        ok.then(|| Currency(code.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Decimal-safe amount tied to a currency. Never a binary float.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Money {
    pub amount: Decimal,
    pub currency: Currency,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Completeness {
    Complete,
    Partial,
    Unavailable,
}

/// Where and when a value came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    /// e.g. `trading212-live`, `trading212-demo`, `mock-fixture`.
    pub source: String,
    /// RFC 3339 UTC time the data was retrieved.
    pub as_of: String,
    pub completeness: Completeness,
    pub stale: bool,
}

/// Account-level figures exactly as reported by the provider, all in the
/// account's primary currency. These are provider-reported, not recomputed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSummary {
    pub currency: Currency,
    pub total_value: Money,
    pub cash_available_to_trade: Money,
    pub cash_in_pies: Money,
    pub cash_reserved_for_orders: Money,
    pub investments_current_value: Money,
    pub investments_total_cost: Money,
    pub investments_unrealized_pl: Money,
    pub investments_realized_pl: Money,
    pub provenance: Provenance,
}

/// An open position. Money fields are in the account currency (provider
/// wallet impact) and are `None` when not supplied; never defaulted to zero.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub ticker: String,
    pub name: Option<String>,
    pub quantity: Decimal,
    pub market_value: Option<Money>,
    pub total_cost: Option<Money>,
    pub unrealized_pl: Option<Money>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Positions {
    pub positions: Vec<Position>,
    pub provenance: Provenance,
}

/// A list read from a paginated history endpoint. `complete` is true only if
/// pagination reached the provider's end (no request budget truncation).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct History<T> {
    pub items: Vec<T>,
    pub provenance: Provenance,
}

/// Cash movement to/from the account (provider `type` kept verbatim).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    pub date_time: String,
    /// DEPOSIT, WITHDRAW, FEE, TRANSFER, INTEREST_ON_FREE_CASH, LENDING_INTEREST, ...
    pub kind: String,
    pub amount: Money,
}

/// Dividend or distribution paid. `amount` is in the account currency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dividend {
    pub paid_on: String,
    pub ticker: String,
    pub kind: String,
    pub amount: Money,
}

/// An executed fill from order history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fill {
    pub filled_at: String,
    pub ticker: String,
    /// BUY or SELL (None if the provider did not supply it).
    pub side: Option<String>,
    pub quantity: Decimal,
    /// Price per share in instrument currency.
    pub price: Option<Decimal>,
    /// Net value in the account currency (provider wallet impact).
    pub net_value: Option<Money>,
    /// Provider-reported realised P&L in the account currency.
    pub realised_pl: Option<Money>,
}
