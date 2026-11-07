//! Minimal read-only MCP server over stdio (newline-delimited JSON-RPC 2.0).
//!
//! Only the tools in [`TOOLS`] exist. Unknown tools and unsupported arguments
//! are rejected before any provider call.

use serde_json::{json, Value};
use std::io::{BufRead, Write};
use t212_core::analytics::allocation;
use t212_core::domain::{History, Money};
use t212_core::error::{ErrorCode, T212Error};
use t212_core::performance::performance;
use t212_core::provider::PortfolioProvider;
use t212_core::timeline::timeline;

/// MCP protocol revision this server speaks. Re-verify against the current spec
/// before release.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// Protocol revisions this server accepts when the client asks for them. The
/// subset used here (tools, resources) is unchanged across these revisions.
pub const SUPPORTED_VERSIONS: &[&str] = &["2025-06-18", "2025-11-25", "2026-07-28"];

/// MCP Apps extension id and UI resource (https://modelcontextprotocol.io/extensions/apps).
pub const UI_EXTENSION: &str = "io.modelcontextprotocol/ui";
pub const UI_MIME: &str = "text/html;profile=mcp-app";
/// Versioned: hosts cache by URI, so bump on breaking HTML changes.
pub const DASHBOARD_URI: &str = "ui://t212-portfolio/dashboard-v2.html";
const DASHBOARD_HTML: &str = include_str!("dashboard.html");

/// The complete, reviewed tool allowlist. Every entry is read-only.
pub const TOOLS: &[&str] = &[
    "get_account_summary",
    "get_positions",
    "get_portfolio_allocation",
    "get_performance",
    "get_transactions",
    "get_dividends",
    "get_fill_history",
    "show_portfolio_dashboard",
];

/// Most recent items returned by list tools (totals cover all items).
pub const MAX_LIST_ITEMS: usize = 200;

fn tool(name: &str, description: &str) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "openWorldHint": false }
    })
}

fn dashboard_tool() -> Value {
    let mut t = tool(
        "show_portfolio_dashboard",
        "Read-only. Shows an interactive dashboard in the chat: key figures, a holdings donut and cumulative deposits, dividends and realised P&L over time. Use when the user asks to see, show, chart or visualise their portfolio. Also returns the same data as text for clients without widgets. May take up to ~2 minutes on first call for long histories.",
    );
    t["_meta"] = json!({
        "ui": { "resourceUri": DASHBOARD_URI },
        "openai/outputTemplate": DASHBOARD_URI,
        "openai/toolInvocation/invoking": "Loading your portfolio…",
        "openai/toolInvocation/invoked": "Portfolio loaded"
    });
    t
}

fn resource_list() -> Value {
    json!({ "resources": [{
        "uri": DASHBOARD_URI,
        "name": "portfolio_dashboard",
        "description": "Read-only portfolio dashboard view",
        "mimeType": UI_MIME
    }]})
}

fn resource_read(params: &Value) -> Result<Value, (i64, String)> {
    match params.get("uri").and_then(Value::as_str) {
        Some(DASHBOARD_URI) => Ok(json!({ "contents": [{
            "uri": DASHBOARD_URI,
            "mimeType": UI_MIME,
            "text": DASHBOARD_HTML,
            // No external origins: the view loads nothing from the network.
            "_meta": { "ui": { "prefersBorder": true, "csp": { "connectDomains": [], "resourceDomains": [] } } }
        }]})),
        _ => Err((-32002, "resource not found".into())),
    }
}

/// Whether the client declared MCP Apps support in `initialize`.
pub fn client_supports_ui(init_params: &Value) -> bool {
    init_params
        .pointer("/capabilities/extensions")
        .and_then(|e| e.get(UI_EXTENSION))
        .and_then(|u| u.get("mimeTypes"))
        .and_then(Value::as_array)
        .is_some_and(|m| m.iter().any(|x| x.as_str() == Some(UI_MIME)))
}

fn tool_list() -> Value {
    json!({ "tools": [
        tool("get_account_summary", "Read-only. Account totals and cash breakdown as reported by Trading 212, in the account currency, with provenance. Descriptive only, not advice."),
        tool("get_positions", "Read-only. Open positions with quantity, market value, cost and unrealized P&L in the account currency. Values missing from the provider are null, never zero."),
        tool("get_portfolio_allocation", "Read-only. Allocation weights and HHI concentration over positions with a market value; cash excluded from the denominator."),
        tool("get_performance", "Read-only. Performance over time: cumulative net deposits series, total gain vs deposits, annualised money-weighted return (XIRR) when history is complete. Each block is available or unavailable with a reason. May take up to ~2 minutes on first call for long histories (rate-limited paging)."),
        tool("get_transactions", "Read-only. Deposits, withdrawals, fees, transfers and interest with totals by type. Returns the most recent 200 items; totals cover the full fetched history."),
        tool("get_dividends", "Read-only. Dividends and distributions paid, in the account currency, with totals by ticker. Returns the most recent 200 items."),
        tool("get_fill_history", "Read-only. Executed fills (buys and sells) with quantity, price, net value and provider-reported realised P&L. Returns the most recent 200 fills. Cannot place or change orders."),
        dashboard_tool(),
    ]})
}

/// Sum money by a key, keeping currencies separate (never mixed).
fn totals<T>(
    items: &[T],
    key: impl Fn(&T) -> String,
    money: impl Fn(&T) -> Option<&Money>,
) -> Vec<Value> {
    use rust_decimal::Decimal;
    use std::collections::BTreeMap;
    let mut m: BTreeMap<(String, String), (Decimal, usize)> = BTreeMap::new();
    for it in items {
        if let Some(mn) = money(it) {
            let e = m
                .entry((key(it), mn.currency.as_str().to_owned()))
                .or_default();
            e.0 += mn.amount;
            e.1 += 1;
        }
    }
    m.into_iter()
        .map(|((k, c), (total, count))| json!({ "key": k, "currency": c, "total": total, "count": count }))
        .collect()
}

/// Most recent `MAX_LIST_ITEMS` by the given date key, newest first.
fn recent<T: Clone + serde::Serialize>(h: &History<T>, date: impl Fn(&T) -> &str) -> Value {
    let mut items = h.items.clone();
    items.sort_by(|a, b| date(b).cmp(date(a)));
    let total = items.len();
    items.truncate(MAX_LIST_ITEMS);
    json!({ "provenance": h.provenance, "items_total": total, "items_returned": items.len(), "items": items })
}

fn run_tool(provider: &dyn PortfolioProvider, name: &str) -> Result<Value, T212Error> {
    let v = t212_core::CONTRACT_VERSION;
    Ok(match name {
        "get_account_summary" => {
            let s = provider.account_summary()?;
            json!({ "contract_version": v, "account_summary": s })
        }
        "get_positions" => json!({ "contract_version": v, "positions": provider.positions()? }),
        "get_portfolio_allocation" => {
            let p = provider.positions()?;
            json!({ "contract_version": v, "provenance": p.provenance, "allocation": allocation(&p.positions) })
        }
        "show_portfolio_dashboard" => {
            let s = provider.account_summary()?;
            let p = provider.positions()?;
            let t = provider.transactions()?;
            let d = provider.dividends()?;
            let f = provider.fills()?;
            json!({
                "contract_version": v,
                "summary": s,
                "allocation": allocation(&p.positions),
                "performance": performance(&s, &t),
                "timeline": timeline(&s.currency, &t, &d, &f),
            })
        }
        "get_performance" => {
            let s = provider.account_summary()?;
            let t = provider.transactions()?;
            json!({
                "contract_version": v,
                "summary_provenance": s.provenance,
                "history_provenance": t.provenance,
                "performance": performance(&s, &t),
            })
        }
        "get_transactions" => {
            let t = provider.transactions()?;
            json!({
                "contract_version": v,
                "totals_by_type": totals(&t.items, |x| x.kind.clone(), |x| Some(&x.amount)),
                "transactions": recent(&t, |x| &x.date_time),
            })
        }
        "get_dividends" => {
            let d = provider.dividends()?;
            json!({
                "contract_version": v,
                "totals_by_ticker": totals(&d.items, |x| x.ticker.clone(), |x| Some(&x.amount)),
                "dividends": recent(&d, |x| &x.paid_on),
            })
        }
        _ => {
            let f = provider.fills()?;
            json!({
                "contract_version": v,
                "realised_pl_by_ticker": totals(&f.items, |x| x.ticker.clone(), |x| x.realised_pl.as_ref()),
                "fills": recent(&f, |x| &x.filled_at),
            })
        }
    })
}

fn call_tool(provider: &dyn PortfolioProvider, params: &Value) -> Result<Value, T212Error> {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    if !TOOLS.contains(&name) {
        return Err(T212Error::new(ErrorCode::UnknownTool, "unknown tool"));
    }
    let has_args = params
        .get("arguments")
        .and_then(Value::as_object)
        .is_some_and(|a| !a.is_empty());
    if has_args {
        return Err(T212Error::new(
            ErrorCode::InvalidParams,
            "this tool takes no arguments",
        ));
    }
    // Provider failures are tool results (isError), so the model can relay the
    // safe, actionable message. Messages never contain credentials.
    Ok(match run_tool(provider, name) {
        Ok(result) => json!({
            "content": [{ "type": "text", "text": result.to_string() }],
            "structuredContent": result,
            "isError": false
        }),
        Err(e) => json!({
            "content": [{ "type": "text", "text": json!({"error": e.code.as_str(), "message": e.message}).to_string() }],
            "isError": true
        }),
    })
}

/// Handle one JSON-RPC message; `None` means no response (notification).
pub fn handle(provider: &dyn PortfolioProvider, msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let respond = |r: Result<Value, (i64, String)>| {
        id.clone().map(|id| match r {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => {
                json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
            }
        })
    };
    match method {
        "initialize" => {
            let params = msg.get("params").cloned().unwrap_or(Value::Null);
            let requested = params.get("protocolVersion").and_then(Value::as_str);
            let version = requested
                .filter(|v| SUPPORTED_VERSIONS.contains(v))
                .unwrap_or(PROTOCOL_VERSION);
            // Diagnostic only (stderr, never stdout): helps confirm whether a
            // client will render the dashboard widget.
            eprintln!(
                "t212: client {} requested protocol {}; MCP Apps UI {}",
                params
                    .pointer("/clientInfo/name")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
                requested.unwrap_or("none"),
                if client_supports_ui(&params) {
                    "supported"
                } else {
                    "not declared"
                }
            );
            respond(Ok(json!({
                "protocolVersion": version,
                "capabilities": {
                    "tools": { "listChanged": false },
                    "resources": { "subscribe": false, "listChanged": false },
                    "extensions": { UI_EXTENSION: {} }
                },
                "serverInfo": { "name": "t212-portfolio", "version": env!("CARGO_PKG_VERSION") }
            })))
        }
        "resources/list" => respond(Ok(resource_list())),
        "resources/templates/list" => respond(Ok(json!({ "resourceTemplates": [] }))),
        "resources/read" => {
            let params = msg.get("params").cloned().unwrap_or(Value::Null);
            respond(resource_read(&params))
        }
        "ping" => respond(Ok(json!({}))),
        "tools/list" => respond(Ok(tool_list())),
        "tools/call" => {
            let params = msg.get("params").cloned().unwrap_or(Value::Null);
            respond(call_tool(provider, &params).map_err(|e| {
                let code = match e.code {
                    ErrorCode::UnknownTool | ErrorCode::InvalidParams => -32602,
                    _ => -32603,
                };
                (code, e.to_string())
            }))
        }
        m if m.starts_with("notifications/") => None,
        _ => respond(Err((-32601, "method not found".into()))),
    }
}

/// Serve until EOF. Malformed lines get a parse error; nothing is logged to stdout.
pub fn serve(
    provider: &dyn PortfolioProvider,
    input: impl BufRead,
    mut output: impl Write,
) -> std::io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle(provider, &msg),
            Err(_) => Some(json!({ "jsonrpc": "2.0", "id": null,
                "error": { "code": -32700, "message": "parse error" } })),
        };
        if let Some(r) = reply {
            writeln!(output, "{r}")?;
            output.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use t212_core::provider::MockProvider;

    fn run(input: &str) -> Vec<Value> {
        let mut out = Vec::new();
        serve(&MockProvider::builtin(), input.as_bytes(), &mut out).unwrap();
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn tool_list_is_exactly_the_allowlist_and_read_only() {
        let r = run(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#);
        let names: Vec<&str> = r[0]["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, TOOLS);
        for n in names {
            for bad in [
                "order", "trade", "buy", "sell", "cancel", "transfer", "http", "request",
            ] {
                assert!(
                    !n.contains(bad),
                    "tool {n} looks like a mutation/proxy tool"
                );
            }
        }
    }

    #[test]
    fn unknown_tool_and_extra_args_rejected() {
        let r = run(concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"place_order"}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_portfolio_allocation","arguments":{"x":1}}}"#
        ));
        assert!(r[0]["error"].is_object());
        assert!(r[1]["error"].is_object());
    }

    #[test]
    fn allocation_call_returns_metadata() {
        let r = run(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_portfolio_allocation","arguments":{}}}"#,
        );
        let s = &r[0]["result"]["structuredContent"];
        assert_eq!(s["allocation"]["status"], "available");
        assert_eq!(s["provenance"]["source"], "mock-fixture");
    }

    #[test]
    fn notifications_get_no_response_and_garbage_gets_parse_error() {
        let r = run("{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\nnot json");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0]["error"]["code"], -32700);
    }
}

#[cfg(test)]
mod ui_tests {
    use super::*;
    use t212_core::provider::MockProvider;

    fn call(msg: Value) -> Value {
        handle(&MockProvider::builtin(), &msg).unwrap()
    }

    #[test]
    fn advertises_ui_extension_and_serves_self_contained_html() {
        let init = call(
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2026-07-28"}}),
        );
        assert_eq!(init["result"]["protocolVersion"], "2026-07-28");
        assert!(init["result"]["capabilities"]["extensions"][UI_EXTENSION].is_object());
        let old = call(
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}),
        );
        assert_eq!(old["result"]["protocolVersion"], PROTOCOL_VERSION);

        let list = call(json!({"jsonrpc":"2.0","id":2,"method":"resources/list"}));
        assert_eq!(list["result"]["resources"][0]["uri"], DASHBOARD_URI);
        let read = call(
            json!({"jsonrpc":"2.0","id":3,"method":"resources/read","params":{"uri":DASHBOARD_URI}}),
        );
        let c = &read["result"]["contents"][0];
        assert_eq!(c["mimeType"], UI_MIME);
        // The SVG namespace URI is an identifier, never fetched.
        let html = c["text"]
            .as_str()
            .unwrap()
            .replace("http://www.w3.org/2000/svg", "");
        // No external scripts, styles, images or network calls.
        for bad in [
            "http://",
            "https://cdn",
            "<script src",
            "fetch(",
            "XMLHttpRequest",
            "WebSocket",
            "innerHTML",
        ] {
            assert!(
                !html.contains(bad),
                "dashboard must be self-contained: found {bad}"
            );
        }
        let missing = call(
            json!({"jsonrpc":"2.0","id":4,"method":"resources/read","params":{"uri":"file:///etc/passwd"}}),
        );
        assert!(missing["error"].is_object());
    }

    #[test]
    fn dashboard_tool_links_ui_and_returns_data() {
        let tools = call(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}));
        let t = tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "show_portfolio_dashboard")
            .unwrap();
        assert_eq!(t["_meta"]["ui"]["resourceUri"], DASHBOARD_URI);
        let r = call(
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"show_portfolio_dashboard"}}),
        );
        let s = &r["result"]["structuredContent"];
        assert_eq!(s["summary"]["currency"], "GBP");
        assert_eq!(s["allocation"]["status"], "available");
        assert_eq!(s["timeline"]["net_deposits"]["total"], "900.00");
        assert!(r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("summary"));
    }

    #[test]
    fn detects_client_ui_capability() {
        let yes = json!({"capabilities":{"extensions":{UI_EXTENSION:{"mimeTypes":[UI_MIME]}}}});
        assert!(client_supports_ui(&yes));
        assert!(!client_supports_ui(&json!({"capabilities":{}})));
    }
}

#[cfg(test)]
mod error_tests {
    use super::*;
    use t212_core::domain::{AccountSummary, Dividend, Fill, Positions, Transaction};

    fn nc<T>() -> Result<T, T212Error> {
        Err(T212Error::new(ErrorCode::NotConfigured, "run `t212 setup`"))
    }

    struct Failing;
    impl PortfolioProvider for Failing {
        fn transactions(&self) -> Result<History<Transaction>, T212Error> {
            nc()
        }
        fn dividends(&self) -> Result<History<Dividend>, T212Error> {
            nc()
        }
        fn fills(&self) -> Result<History<Fill>, T212Error> {
            nc()
        }
        fn account_summary(&self) -> Result<AccountSummary, T212Error> {
            Err(T212Error::new(ErrorCode::NotConfigured, "run `t212 setup`"))
        }
        fn positions(&self) -> Result<Positions, T212Error> {
            Err(T212Error::new(ErrorCode::NotConfigured, "run `t212 setup`"))
        }
    }

    #[test]
    fn provider_errors_become_safe_tool_errors() {
        let msg =
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_positions"}});
        let r = handle(&Failing, &msg).unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert!(r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("not_configured"));
    }
}
