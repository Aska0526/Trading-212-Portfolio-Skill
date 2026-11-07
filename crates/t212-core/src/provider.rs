//! Provider port. Adapters (mock, Trading 212) implement this and return
//! normalized models only. The trait exposes reads only by construction.

use crate::domain::{AccountSummary, Dividend, Fill, History, Positions, Transaction};
use crate::error::{ErrorCode, T212Error};
use serde::Deserialize;

pub trait PortfolioProvider {
    fn account_summary(&self) -> Result<AccountSummary, T212Error>;
    fn positions(&self) -> Result<Positions, T212Error>;
    fn transactions(&self) -> Result<History<Transaction>, T212Error>;
    fn dividends(&self) -> Result<History<Dividend>, T212Error>;
    fn fills(&self) -> Result<History<Fill>, T212Error>;
}

#[derive(Deserialize)]
struct Fixture {
    contract_version: String,
    account: AccountSummary,
    positions: Positions,
    transactions: History<Transaction>,
    dividends: History<Dividend>,
    fills: History<Fill>,
}

/// Deterministic provider backed by a sanitized, synthetic fixture.
pub struct MockProvider {
    fixture: String,
}

impl MockProvider {
    pub fn from_json(fixture: impl Into<String>) -> Self {
        Self {
            fixture: fixture.into(),
        }
    }

    /// The built-in synthetic fixture (no real account data).
    pub fn builtin() -> Self {
        Self::from_json(include_str!("../../../tests/fixtures/portfolio_basic.json"))
    }

    fn load(&self) -> Result<Fixture, T212Error> {
        let f: Fixture = serde_json::from_str(&self.fixture).map_err(|_| {
            T212Error::new(
                ErrorCode::CompatibilityError,
                "fixture does not match the normalized contract",
            )
        })?;
        if f.contract_version != crate::CONTRACT_VERSION {
            return Err(T212Error::new(
                ErrorCode::CompatibilityError,
                "fixture contract version mismatch",
            ));
        }
        Ok(f)
    }
}

impl PortfolioProvider for MockProvider {
    fn account_summary(&self) -> Result<AccountSummary, T212Error> {
        Ok(self.load()?.account)
    }
    fn positions(&self) -> Result<Positions, T212Error> {
        Ok(self.load()?.positions)
    }
    fn transactions(&self) -> Result<History<Transaction>, T212Error> {
        Ok(self.load()?.transactions)
    }
    fn dividends(&self) -> Result<History<Dividend>, T212Error> {
        Ok(self.load()?.dividends)
    }
    fn fills(&self) -> Result<History<Fill>, T212Error> {
        Ok(self.load()?.fills)
    }
}
