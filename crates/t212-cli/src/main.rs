//! `t212` command line interface.

mod install;
mod report;

use install::Client;
use std::io::{stdin, stdout, BufRead, Write};
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use t212_api::{Credentials, Environment, T212Provider, UreqTransport};
use t212_core::domain::{AccountSummary, Dividend, Fill, History, Positions, Transaction};
use t212_core::error::{ErrorCode, T212Error};
use t212_core::provider::{MockProvider, PortfolioProvider};
use t212_creds::{CredentialStore, OsStore, Stored};

const USAGE: &str = "usage:
  t212 setup [--skip-verify]       store Trading 212 API credentials in the OS keychain
  t212 status                      check credentials and connectivity (prints no secrets)
  t212 install --client <claude-code|chatgpt> [--yes]
  t212 uninstall --client <claude-code|chatgpt>
  t212 report [--mock]             portfolio overview and performance in the terminal
  t212 logout                      delete stored credentials
  t212 mcp [--mock]                run the read-only MCP server on stdio
  t212 version";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    match a.as_slice() {
        ["version"] | ["--version"] => {
            println!(
                "t212 {} (contract {})",
                env!("CARGO_PKG_VERSION"),
                t212_core::CONTRACT_VERSION
            );
            ExitCode::SUCCESS
        }
        ["mcp", "--mock"] => serve(&MockProvider::builtin()),
        ["mcp"] => serve(&LazyLive::default()),
        ["report", "--mock"] => report::print(&MockProvider::builtin()),
        ["report"] => report::print(&LazyLive::default()),
        ["setup"] => setup(false),
        ["setup", "--skip-verify"] => setup(true),
        ["status"] => status(),
        ["logout"] => match OsStore.delete() {
            Ok(true) => say(t(
                "Stored credentials deleted from the OS keychain.",
                "已从系统钥匙串删除保存的密钥。",
            )),
            Ok(false) => say(t("No stored credentials found.", "未找到已保存的密钥。")),
            Err(e) => fail(&e.to_string()),
        },
        ["install", rest @ ..] => client_cmd(rest, true),
        ["uninstall", rest @ ..] => client_cmd(rest, false),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// Chinese output when `T212_LANG=zh` or the locale is Chinese. The
/// installer sets `T212_LANG` from the macOS system language.
fn zh() -> bool {
    ["T212_LANG", "LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .is_some_and(|v| v.to_ascii_lowercase().starts_with("zh"))
}

/// Pick the English or Chinese string.
fn t(en: &'static str, cn: &'static str) -> &'static str {
    if zh() {
        cn
    } else {
        en
    }
}

fn say(m: &str) -> ExitCode {
    println!("{m}");
    ExitCode::SUCCESS
}
fn fail(m: &str) -> ExitCode {
    eprintln!("t212: {m}");
    ExitCode::FAILURE
}

/// Live provider that reads the keychain on the first tool call, not at
/// startup, so client discovery (initialize, tools/list) never triggers a
/// keychain prompt.
#[derive(Default)]
struct LazyLive(std::cell::OnceCell<Result<T212Provider<UreqTransport>, T212Error>>);

impl LazyLive {
    fn get(&self) -> Result<&T212Provider<UreqTransport>, T212Error> {
        self.0
            .get_or_init(|| match OsStore.load() {
                Ok(Some(s)) => Ok(T212Provider::new(UreqTransport::default(), s.environment, s.credentials)),
                Ok(None) => Err(T212Error::new(ErrorCode::NotConfigured, "Trading 212 is not set up. Ask the user to run `t212 setup` in their own terminal. Never ask for credentials in chat.")),
                Err(e) => Err(T212Error::new(e.code, "the OS secure credential store is unavailable or access was denied; ask the user to run `t212 status` in their terminal.")),
            })
            .as_ref()
            .map_err(Clone::clone)
    }
}

impl PortfolioProvider for LazyLive {
    fn account_summary(&self) -> Result<AccountSummary, T212Error> {
        self.get()?.account_summary()
    }
    fn positions(&self) -> Result<Positions, T212Error> {
        self.get()?.positions()
    }
    fn transactions(&self) -> Result<History<Transaction>, T212Error> {
        self.get()?.transactions()
    }
    fn dividends(&self) -> Result<History<Dividend>, T212Error> {
        self.get()?.dividends()
    }
    fn fills(&self) -> Result<History<Fill>, T212Error> {
        self.get()?.fills()
    }
}

fn serve(p: &dyn PortfolioProvider) -> ExitCode {
    // stdout is the protocol channel; diagnostics go to stderr only.
    match t212_mcp::serve(p, stdin().lock(), stdout().lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

fn prompt_line(label: &str) -> String {
    eprint!("{label}");
    let _ = std::io::stderr().flush();
    let mut s = String::new();
    let _ = stdin().lock().read_line(&mut s);
    s.trim().to_owned()
}

fn setup(skip_verify: bool) -> ExitCode {
    eprintln!(
        "{}",
        t(
            "T212 Portfolio setup\n\
             Create an API key in Trading 212: Settings > API (Beta) > Generate API key.\n\
             Enable READ permissions only (account data, portfolio, history). Do NOT enable orders.\n\
             Your key and secret are typed here with hidden input and saved to your system keychain.\n\
             They are never shown to any AI assistant.\n",
            "T212 Portfolio 设置\n\
             在 Trading 212 中创建 API 密钥：设置 > API (Beta) > 生成 API 密钥。\n\
             只开启读取权限（账户数据、投资组合、历史记录），不要开启下单权限。\n\
             密钥在此处隐藏输入，并保存到系统钥匙串中，\n\
             不会显示给任何 AI 助手。\n",
        )
    );
    let env = match prompt_line(t(
        "Account [live/demo] (default live): ",
        "账户类型 [live 实盘 / demo 模拟]（默认 live）：",
    ))
    .as_str()
    {
        "" | "live" => Environment::Live,
        "demo" => Environment::Demo,
        _ => return fail(t("please enter `live` or `demo`", "请输入 live 或 demo")),
    };
    let key = match rpassword::prompt_password(t("API key (hidden): ", "API 密钥（隐藏输入）："))
    {
        Ok(k) if !k.trim().is_empty() => k.trim().to_owned(),
        _ => return fail(t("no API key entered", "未输入 API 密钥")),
    };
    let secret =
        match rpassword::prompt_password(t("API secret (hidden): ", "API Secret（隐藏输入）："))
        {
            Ok(k) if !k.trim().is_empty() => k.trim().to_owned(),
            _ => return fail(t("no API secret entered", "未输入 API Secret")),
        };
    let stored = Stored {
        credentials: Credentials { key, secret },
        environment: env,
    };

    if !skip_verify {
        let p = T212Provider::new(UreqTransport::default(), env, stored.credentials.clone());
        match p.account_summary() {
            Ok(s) => eprintln!(
                "{} {}",
                t("Verified. Account currency:", "验证成功。账户货币："),
                s.currency.as_str()
            ),
            Err(e) => {
                return fail(&format!(
                    "{} ({})",
                    t(
                        "Could not verify the key; nothing was saved. Check the key, secret and account type",
                        "无法验证密钥，未保存任何内容。请检查密钥、Secret 和账户类型",
                    ),
                    e.code.as_str()
                ))
            }
        }
    }
    if let Err(e) = OsStore.save(&stored) {
        return fail(&e.to_string());
    }
    eprintln!(
        "{}",
        t("Saved to your system keychain.", "已保存到系统钥匙串。")
    );
    say(t("Setup complete.", "设置完成。"))
}

fn status() -> ExitCode {
    let stored = match OsStore.load() {
        Ok(Some(s)) => s,
        Ok(None) => {
            return fail(t(
                "not set up. Run `t212 setup`.",
                "尚未设置，请运行 `t212 setup`。",
            ))
        }
        Err(e) => return fail(&e.to_string()),
    };
    println!(
        "credentials: stored in OS keychain (environment: {})",
        stored.environment.as_str()
    );
    let p = T212Provider::new(
        UreqTransport::default(),
        stored.environment,
        stored.credentials,
    );
    match p.account_summary() {
        Ok(s) => say(&format!(
            "connection: ok (account currency {})",
            s.currency.as_str()
        )),
        Err(e) => fail(&format!("connection: {} - {}", e.code.as_str(), e.message)),
    }
}

fn confirm(prompt: &str) -> bool {
    matches!(
        prompt_line(prompt).to_lowercase().as_str(),
        "y" | "yes" | "是"
    )
}

fn home() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| "cannot determine home directory".to_string())
}

fn client_cmd(rest: &[&str], installing: bool) -> ExitCode {
    let (mut client, mut yes) = (None, false);
    let mut it = rest.iter();
    while let Some(&arg) = it.next() {
        match arg {
            "--client" => client = it.next().and_then(|c| Client::parse(c)),
            "--yes" | "-y" => yes = true,
            _ => {}
        }
    }
    let Some(client) = client else {
        return fail("--client must be `claude-code` or `chatgpt`");
    };
    let home = match home() {
        Ok(h) => h,
        Err(e) => return fail(&e),
    };
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(_) => return fail("cannot locate the t212 binary"),
    };
    if installing {
        run_install(client, &home, &exe, yes)
    } else {
        run_uninstall(client, &home)
    }
}

fn run_install(
    client: Client,
    home: &std::path::Path,
    exe: &std::path::Path,
    yes: bool,
) -> ExitCode {
    let plan = match client {
        Client::ClaudeCode => install::claude_plan(home, exe, zh()),
        Client::ChatGpt => install::chatgpt_plan(home, zh()),
    };
    println!("{}", t("This will:", "将执行以下操作："));
    for step in &plan {
        println!("  - {step}");
    }
    if !yes && !confirm(t("Proceed? [y/N] ", "是否继续？[y/N] ")) {
        return fail(t("cancelled; nothing changed", "已取消，未做任何更改"));
    }
    match client {
        Client::ClaudeCode => {
            if let Err(e) = install::write_skill(&install::claude_skill_dir(home)) {
                return fail(&e);
            }
            let out = Command::new("claude")
                .args(["mcp", "add", "--scope", "user", install::NAME, "--"])
                .arg(exe)
                .arg("mcp")
                .status();
            match out {
                Ok(s) if s.success() => say(t(
                    "Installed. Restart Claude Code, then ask about your portfolio.",
                    "安装完成。重启 Claude Code 后即可询问你的投资组合。",
                )),
                _ => fail(t(
                    "skill installed, but `claude mcp add` failed or `claude` is not on PATH. Run manually:\n  claude mcp add --scope user t212-portfolio -- <path-to-t212> mcp",
                    "Skill 已安装，但 `claude mcp add` 失败或找不到 `claude` 命令。请手动运行：\n  claude mcp add --scope user t212-portfolio -- <t212 路径> mcp",
                )),
            }
        }
        Client::ChatGpt => match install::install_chatgpt(home, exe) {
            Ok(()) => say(t(
                "Installed. Next:\n  1. Fully quit and reopen the ChatGPT desktop app.\n  2. Open Plugins > Personal and install \"t212-portfolio\".\n  3. Use it in a Work or Codex thread. Regular Chat threads can't reach your computer.",
                "安装完成。接下来：\n  1. 完全退出并重新打开 ChatGPT 桌面应用。\n  2. 打开 插件 > 个人，安装 \"t212-portfolio\"。\n  3. 在 Work 或 Codex 对话中使用。普通 Chat 对话无法访问你的电脑。",
            )),
            Err(e) => fail(&e),
        },
    }
}

fn run_uninstall(client: Client, home: &std::path::Path) -> ExitCode {
    match client {
        Client::ClaudeCode => {
            let _ = Command::new("claude")
                .args(["mcp", "remove", "--scope", "user", install::NAME])
                .status();
            match install::uninstall_claude_skill(home) {
                Ok(_) => say(t(
                    "Removed. Your saved key was kept; run `t212 logout` to delete it.",
                    "已移除。保存的密钥仍保留，运行 `t212 logout` 可删除。",
                )),
                Err(e) => fail(&e),
            }
        }
        Client::ChatGpt => match install::uninstall_chatgpt(home) {
            Ok(done) => say(&format!(
                "{}\n{}",
                done.join("\n"),
                t(
                    "Your saved key was kept; run `t212 logout` to delete it.",
                    "保存的密钥仍保留，运行 `t212 logout` 可删除。",
                )
            )),
            Err(e) => fail(&e),
        },
    }
}
