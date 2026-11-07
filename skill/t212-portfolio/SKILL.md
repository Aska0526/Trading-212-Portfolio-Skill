---
name: t212-portfolio
description: Read and explain a user's Trading 212 portfolio using the local read-only T212 Portfolio MCP tools. Use for holdings, account summaries, allocation, concentration, performance over time, returns, dividends, deposits, transactions and past trades.
---

# T212 Portfolio

Use the local T212 Portfolio MCP server to answer questions about the connected account. Follow the user's language; preserve ticker symbols, currency codes, and metric names where useful.

## Tools

The local server exposes eight read-only tools. None take arguments. Call only the ones the question needs.

| Question | Tool |
|---|---|
| Account value, cash, total P&L | `get_account_summary` |
| Holdings, cost, unrealised P&L per position | `get_positions` |
| Allocation weights, concentration | `get_portfolio_allocation` |
| Performance over time, returns, "how am I doing" | `get_performance` |
| Deposits, withdrawals, fees, interest | `get_transactions` |
| Dividend income | `get_dividends` |
| Past buys and sells, realised P&L by ticker | `get_fill_history` |
| "Show", "chart", "visualise" or a portfolio overview | `show_portfolio_dashboard` |

Trading 212 rate-limits history reads (20 requests per minute), so the first history or performance call on a long account can take up to about two minutes. Results are cached in the session for five minutes. On `rate_limited`, wait and retry once rather than looping.

## Showing results

When the user wants to see their portfolio, or asks a broad "how am I doing" question, call `show_portfolio_dashboard`. In clients that support MCP Apps it renders an interactive dashboard (key figures, holdings donut, cumulative deposits, dividends and realised P&L over time). Do not repeat every number the dashboard shows; add a short interpretation in the user's language instead. If no dashboard appears, the tool result still contains the full data: summarise it with a compact markdown table.

For narrower questions, use the specific tool and answer with short text or a small table. Use `get_performance` and the `timeline` block from the dashboard tool for over-time questions.

## Performance and time-series questions

Trading 212 does not provide historical portfolio valuations or historical prices. `get_performance` returns only what the data supports, each block marked `available` or `unavailable` with a reason:

- `net_contributions`: cumulative deposits minus withdrawals by date. This is money put in, not portfolio value.
- `total_gain`: current total value minus net contributions.
- `money_weighted_return`: annualised XIRR. It is an estimate; always mention the period and any `caveat` (for example, periods under a year).

For "last 24 hours" or "today" questions, say plainly that Trading 212 does not provide past account values, so daily change is not available. You can still report what happened in that period from history: deposits, withdrawals, dividends and executed trades. Never estimate daily moves, invent prices, or substitute market news. Do not present the net-contributions series as portfolio value.

## Credentials and setup

Never ask the user to type, paste or share an API key or secret in chat. If a tool returns `not_configured` or `authentication_failed`, tell the user to run `t212 setup` in their own terminal (the Terminal app, not this chat), where the secret is entered with hidden input and stored in the OS keychain. Advise creating the key with read-only permissions only (account data, portfolio, history; no orders).

## Safe workflow

1. Identify the account, period, currency, or metric definition only when it materially affects the answer.
2. Call the smallest relevant read-only tool set. Never ask for, copy, or transmit API credentials.
3. Read the returned `as_of`, currency, source, completeness, staleness, and calculation metadata before interpreting values.
4. Explain the metric in plain language and distinguish observed values from derived calculations.
5. If the data is missing, partial, stale, unsupported, or inconsistent, state that plainly. Do not replace missing values with zero or infer FX, prices, cost basis, fees, tax, or corporate actions.
6. Keep answers descriptive. Do not recommend buying, selling, or changing allocations.

## Read-only boundary

This integration cannot place, change, or cancel orders; transfer funds; or modify a Trading 212 account. For any request to act on the account, explain the boundary and direct the user to Trading 212's own application. Do not formulate an order or suggest that MCP can execute it.

## Analytics interpretation

- State the valuation timestamp and base currency for portfolio totals.
- State the denominator used for allocation or concentration and whether cash is included.
- Keep realised and unrealised P&L separate. Account-level and per-fill realised P&L are provider-reported; say so.
- Report the money-weighted return only when `get_performance` marks it available, with its period and the word "estimate". Time-weighted return is not computed.
- Dividend amounts are in the account currency as reported; do not infer gross/net or tax treatment.
- Do not imply that historical performance predicts future returns.

## Privacy

Never repeat account identifiers or sensitive raw transaction details unless the user asks and the detail is necessary. Do not expose credential values, local secret-store locations, or raw provider payloads. Do not send portfolio data to external services.

## References

Detailed metric definitions and unsupported cases are in `references/metrics.md`. Follow the server's returned definition version if it differs from this reference.
