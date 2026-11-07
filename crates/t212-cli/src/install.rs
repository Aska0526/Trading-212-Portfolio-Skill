//! Client integrations. Each one previews exactly what it will change, only
//! touches product-owned entries, and preserves everything else.
//!
//! Verified against official docs on 2026-11-07:
//! - Claude Code: personal skills in `~/.claude/skills/<name>/SKILL.md`
//!   (https://code.claude.com/docs/en/skills); local stdio MCP via
//!   `claude mcp add --scope user <name> -- <command>`
//!   (https://code.claude.com/docs/en/mcp).
//! - ChatGPT desktop / Codex: personal marketplace at
//!   `~/.agents/plugins/marketplace.json`, plugin with root `plugin.json`,
//!   `mcp.json` (stdio) and `skills/`
//!   (https://developers.openai.com/plugins/build/plugins).

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub const NAME: &str = "t212-portfolio";
const MARKER: &str = ".t212-portfolio-owned";

const SKILL_MD: &str = include_str!("../../../skill/t212-portfolio/SKILL.md");
const METRICS_MD: &str = include_str!("../../../skill/t212-portfolio/references/metrics.md");

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Client {
    ClaudeCode,
    ChatGpt,
}

impl Client {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "claude-code" | "claude" => Some(Client::ClaudeCode),
            "chatgpt" | "codex" => Some(Client::ChatGpt),
            _ => None,
        }
    }
}

fn write(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    fs::write(path, contents).map_err(|e| format!("write {}: {e}", path.display()))
}

pub fn write_skill(dir: &Path) -> Result<(), String> {
    write(&dir.join("SKILL.md"), SKILL_MD)?;
    write(&dir.join("references/metrics.md"), METRICS_MD)?;
    write(
        &dir.join(MARKER),
        "created by `t212 install`; safe to remove with `t212 uninstall`\n",
    )
}

fn owned(dir: &Path) -> bool {
    dir.join(MARKER).exists()
}

fn remove_owned(dir: &Path) -> Result<bool, String> {
    if !dir.exists() {
        return Ok(false);
    }
    if !owned(dir) {
        return Err(format!(
            "{} exists but was not created by t212; leaving it alone",
            dir.display()
        ));
    }
    fs::remove_dir_all(dir).map_err(|e| format!("remove {}: {e}", dir.display()))?;
    Ok(true)
}

// ---------- Claude Code ----------

pub fn claude_skill_dir(home: &Path) -> PathBuf {
    home.join(".claude/skills").join(NAME)
}

pub fn claude_plan(home: &Path, exe: &Path, zh: bool) -> Vec<String> {
    let (write, run) = if zh {
        ("写入 Skill 文件到", "运行：")
    } else {
        ("write skill files to", "run:")
    };
    vec![
        format!("{write} {}", claude_skill_dir(home).display()),
        format!(
            "{run} claude mcp add --scope user {NAME} -- {} mcp",
            exe.display()
        ),
    ]
}

// ---------- ChatGPT desktop / Codex ----------

pub fn chatgpt_plugin_dir(home: &Path) -> PathBuf {
    home.join(".codex/plugins").join(NAME)
}

pub fn chatgpt_marketplace_path(home: &Path) -> PathBuf {
    home.join(".agents/plugins/marketplace.json")
}

pub fn chatgpt_plan(home: &Path, zh: bool) -> Vec<String> {
    let plugin = chatgpt_plugin_dir(home).display().to_string();
    let mp = chatgpt_marketplace_path(home).display().to_string();
    if zh {
        vec![
            format!("写入插件到 {plugin}"),
            format!("在 {mp} 中添加 \"{NAME}\"（保留其他条目，并备份原文件）"),
        ]
    } else {
        vec![
            format!("write plugin to {plugin}"),
            format!("add a \"{NAME}\" entry to {mp} (other entries kept, backup saved)"),
        ]
    }
}

pub fn plugin_manifest() -> Value {
    json!({
        "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
        "name": NAME,
        "version": PLUGIN_VERSION,
        "description": "Read-only Trading 212 portfolio analysis. Credentials stay in your OS keychain.",
        "extensions": { "com.openai": { "requires_local_executor": true } }
    })
}

/// Path of the bundled binary inside the plugin. ChatGPT requires a bare
/// command name or a contained `./` path ("Agent Plugins stdio command must be
/// a bare executable name or a contained `./` path"), so absolute paths fail.
pub const BUNDLED_BIN: &str = "./bin/t212";

pub fn plugin_mcp() -> Value {
    json!({
        "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
        "mcpServers": { NAME: {
        "type": "stdio",
        "command": BUNDLED_BIN,
        "args": ["mcp"]
    }}})
}

/// Version written into plugin.json. Bumped whenever the generated package
/// changes so the ChatGPT desktop app refreshes its cached copy.
pub const PLUGIN_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Insert or replace only our own entry; every other key and plugin is kept.
pub fn merge_marketplace(existing: Option<Value>) -> Result<Value, String> {
    let mut root = match existing {
        Some(v) if v.is_object() => v,
        Some(_) => {
            return Err("existing marketplace.json is not a JSON object; refusing to modify".into())
        }
        None => {
            json!({ "name": "personal", "interface": { "displayName": "Personal" }, "plugins": [] })
        }
    };
    let obj = root.as_object_mut().unwrap();
    let plugins = obj.entry("plugins").or_insert_with(|| json!([]));
    let arr = plugins
        .as_array_mut()
        .ok_or("existing marketplace.json has a non-array \"plugins\"; refusing to modify")?;
    arr.retain(|p| p.get("name").and_then(Value::as_str) != Some(NAME));
    arr.push(json!({
        "name": NAME,
        "source": { "source": "local", "path": format!("./.codex/plugins/{NAME}") },
        "policy": { "installation": "AVAILABLE", "authentication": "ON_INSTALL" },
        "category": "Productivity"
    }));
    Ok(root)
}

pub fn remove_from_marketplace(mut root: Value) -> Value {
    if let Some(arr) = root.get_mut("plugins").and_then(Value::as_array_mut) {
        arr.retain(|p| p.get("name").and_then(Value::as_str) != Some(NAME));
    }
    root
}

pub fn install_chatgpt(home: &Path, exe: &Path) -> Result<(), String> {
    let dir = chatgpt_plugin_dir(home);
    write(&dir.join("plugin.json"), &pretty(&plugin_manifest()))?;
    write(&dir.join("mcp.json"), &pretty(&plugin_mcp()))?;
    let bin = dir.join("bin/t212");
    fs::create_dir_all(bin.parent().unwrap()).map_err(|e| format!("create bin dir: {e}"))?;
    fs::copy(exe, &bin).map_err(|e| format!("copy binary into plugin: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("chmod: {e}"))?;
    }
    write_skill(&dir.join("skills").join(NAME))?;
    write(&dir.join(MARKER), "created by `t212 install`\n")?;

    let mp = chatgpt_marketplace_path(home);
    let existing =
        match fs::read_to_string(&mp) {
            Ok(raw) => {
                let backup = mp.with_extension("json.t212-backup");
                fs::write(&backup, &raw).map_err(|e| format!("backup: {e}"))?;
                Some(serde_json::from_str(&raw).map_err(|_| {
                    "existing marketplace.json is not valid JSON; refusing to modify"
                })?)
            }
            Err(_) => None,
        };
    write(&mp, &pretty(&merge_marketplace(existing)?))
}

pub fn uninstall_chatgpt(home: &Path) -> Result<Vec<String>, String> {
    let mut done = vec![];
    if remove_owned(&chatgpt_plugin_dir(home))? {
        done.push(format!("removed {}", chatgpt_plugin_dir(home).display()));
    }
    let mp = chatgpt_marketplace_path(home);
    if let Ok(raw) = fs::read_to_string(&mp) {
        let v: Value = serde_json::from_str(&raw)
            .map_err(|_| "marketplace.json is not valid JSON; left untouched")?;
        write(&mp, &pretty(&remove_from_marketplace(v)))?;
        let _ = fs::remove_file(mp.with_extension("json.t212-backup"));
        done.push(format!("removed {NAME} entry from {}", mp.display()));
    }
    Ok(done)
}

pub fn uninstall_claude_skill(home: &Path) -> Result<bool, String> {
    remove_owned(&claude_skill_dir(home))
}

fn pretty(v: &Value) -> String {
    let mut s = serde_json::to_string_pretty(v).unwrap();
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marketplace_merge_preserves_other_plugins_and_is_idempotent() {
        let existing = json!({"name":"mine","extra":1,"plugins":[{"name":"other","x":true}]});
        let once = merge_marketplace(Some(existing)).unwrap();
        let twice = merge_marketplace(Some(once.clone())).unwrap();
        assert_eq!(once, twice);
        assert_eq!(once["extra"], 1);
        let names: Vec<_> = once["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["other", NAME]);
        let removed = remove_from_marketplace(once);
        assert_eq!(removed["plugins"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn refuses_malformed_marketplace() {
        assert!(merge_marketplace(Some(json!([1]))).is_err());
        assert!(merge_marketplace(Some(json!({"plugins": 5}))).is_err());
    }

    #[test]
    fn install_and_uninstall_roundtrip_leaves_foreign_files() {
        let home = std::env::temp_dir().join(format!("t212-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let mp = chatgpt_marketplace_path(&home);
        write(&mp, r#"{"name":"mine","plugins":[{"name":"other"}]}"#).unwrap();
        let fake_exe = home.join("fake-t212");
        write(&fake_exe, "#!/bin/sh\n").unwrap();
        install_chatgpt(&home, &fake_exe).unwrap();
        assert!(chatgpt_plugin_dir(&home).join("mcp.json").exists());
        assert!(chatgpt_plugin_dir(&home).join("bin/t212").exists());
        assert!(chatgpt_plugin_dir(&home)
            .join("skills/t212-portfolio/SKILL.md")
            .exists());
        uninstall_chatgpt(&home).unwrap();
        assert!(!chatgpt_plugin_dir(&home).exists());
        let v: Value = serde_json::from_str(&fs::read_to_string(&mp).unwrap()).unwrap();
        assert_eq!(v["plugins"].as_array().unwrap().len(), 1);
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn will_not_delete_unowned_skill_dir() {
        let home = std::env::temp_dir().join(format!("t212-test2-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        write(&claude_skill_dir(&home).join("SKILL.md"), "user's own").unwrap();
        assert!(uninstall_claude_skill(&home).is_err());
        assert!(claude_skill_dir(&home).join("SKILL.md").exists());
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn mcp_entry_contains_no_credentials_and_only_mcp_arg() {
        let v = plugin_mcp();
        // ChatGPT desktop rejects the MCP config without this field
        // ("failed to parse plugin MCP config: missing field `$schema`").
        assert_eq!(
            v["$schema"],
            "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json"
        );
        assert_eq!(e_type(&v), "stdio");
        let cmd = v["mcpServers"][NAME]["command"].as_str().unwrap();
        assert!(
            cmd.starts_with("./") && !cmd.contains(".."),
            "command must be a contained ./ path"
        );
        let e = &v["mcpServers"][NAME];
        assert_eq!(e["args"], json!(["mcp"]));
        assert!(e.get("env").is_none());
    }

    fn e_type(v: &Value) -> &str {
        v["mcpServers"][NAME]["type"].as_str().unwrap()
    }
}
