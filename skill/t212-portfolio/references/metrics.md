# Metric interpretation reference

This reference states presentation rules. The server's versioned calculation metadata (`allocation_v1`, `performance_v1`) is authoritative for any displayed result.

- **Allocation weight:** position market value divided by the sum of included position market values. Cash is excluded. Positions without a market value are excluded and listed.
- **Concentration (HHI):** sum of squared fractional weights over the included positions. 1.0 means a single holding; lower means more spread. Do not compare values unless the universe and denominator match.
- **Unrealised P&L:** provider-reported (`walletImpact.unrealizedProfitLoss` = current value minus total cost, in the account currency). Say it is provider-reported.
- **Realised P&L:** provider-reported at account level and per fill. Do not recompute it or claim tax treatment.
- **Dividends:** amounts in the account currency with paid date and provider type. Do not infer gross/net or withholding tax.
- **Net contributions:** cumulative DEPOSIT minus WITHDRAW, plus TRANSFER items by the sign of their amount, in the account currency by UTC date. Trading 212 does not document the transfer sign; positive is assumed to mean money into the account. When `transfers_counted` is above zero, mention that assumption. Fees, interest, dividends and trades are internal, not external flows. Unavailable if history is partial or mixes currencies.
- **Gain vs deposits:** current total value minus net contributions; the percentage uses net contributions as the denominator.
- **Money-weighted return (XIRR):** annualised rate that equates deposits (outflows), withdrawals (inflows) and current total value (terminal inflow); day precision, 365-day year. Always an estimate; state the period. Under a year, warn that annualising exaggerates short-term moves.
- **Time-weighted return (TWR):** not computed. It needs a valuation at every external cash flow, which the API does not provide.
- **Missing or partial data:** preserve null/unavailable status; never convert it to zero.
