//! VS Code with GitHub Copilot: custom agents in `<copilot>/agents/*.agent.md`, the
//! delegation guidance in `<copilot>/copilot-instructions.md` and a `SubagentStop`
//! hook in `<copilot>/hooks/vario.json`, for every workspace. `<copilot>` is
//! `$COPILOT_HOME`, or `~/.copilot`.
//!
//! VS Code delegates through its `agent` tool set (`runSubagent`), on its own or when
//! asked; hooks are a preview that `chat.useHooks` turns on. A sub-agent cannot use a
//! model more expensive than the main one, and models are named as the model picker
//! shows them.

use std::path::{Path, PathBuf};
use std::{fs, time::SystemTime};

use serde_json::{Map, Value, json};

use super::subagents::{self, MARKER, Subagent, Tier, yaml_string};
use super::{Env, Integration, has_mcp_server};
use crate::active::Active;
use crate::handoff;
use crate::parts::{JsonHooks, Kind, MarkdownBlock, OwnFile, Part};
use crate::paths::Os;

pub fn integration() -> Integration {
    Integration {
        name: "vscode",
        label: "VS Code (Copilot)",
        detect: |env| env.which("code").is_some() || user_dir(env).is_dir(),
        parts,
        handoff_connected: |env| {
            has_mcp_server(
                &user_dir(env).join("mcp.json"),
                "servers",
                handoff::SERVER_NAME,
            )
        },
    }
}

fn copilot_dir(env: &Env) -> PathBuf {
    env.var("COPILOT_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| env.home.join(".copilot"))
}

/// VS Code's own user settings folder, where its user-level `mcp.json` lives.
fn user_dir(env: &Env) -> PathBuf {
    match Os::current() {
        Os::MacOs => env.home.join("Library/Application Support/Code/User"),
        Os::Windows => env
            .var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| env.home.join("AppData").join("Roaming"))
            .join("Code")
            .join("User"),
        Os::Other => env
            .var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| env.home.join(".config"))
            .join("Code")
            .join("User"),
    }
}

/// Claude Haiku is included in Copilot plans; the intermediate tier keeps the model
/// picked for the chat.
fn model(tier: Tier) -> Option<&'static str> {
    match tier {
        Tier::Economical => Some("Claude Haiku 4.5"),
        Tier::Intermediate => None,
    }
}

fn parts(env: &Env) -> Vec<Box<dyn Part>> {
    let dir = copilot_dir(env);
    let mut parts: Vec<Box<dyn Part>> = subagents::all()
        .iter()
        .map(|subagent| -> Box<dyn Part> {
            Box::new(OwnFile {
                path: dir
                    .join("agents")
                    .join(format!("{}.agent.md", subagent.name)),
                content: render(subagent),
                kind: Kind::Subagents,
                marker: Some(MARKER.into()),
            })
        })
        .collect();
    parts.push(Box::new(MarkdownBlock {
        path: dir.join("copilot-instructions.md"),
        body: subagents::INSTRUCTIONS.into(),
    }));
    parts.push(Box::new(JsonHooks {
        path: dir.join("hooks").join("vario.json"),
        keys: vec!["hooks".into()],
        entries: vec![
            (
                "SessionStart".into(),
                json!({
                    "type": "command",
                    "command": env.hook_command("session-start", "vscode"),
                    "timeout": 5,
                }),
            ),
            (
                "SubagentStop".into(),
                json!({
                    "type": "command",
                    "command": env.hook_command("subagent-stop", "vscode"),
                    "timeout": 10,
                }),
            ),
            // Copilot's hooks carry no model nor tokens; this one tells the gauge that
            // Copilot is the client in use, and it reads the model from the local store.
            (
                "Stop".into(),
                json!({
                    "type": "command",
                    "command": env.hook_command("activity", "vscode"),
                    "timeout": 5,
                }),
            ),
        ],
        defaults: Map::new(),
        kind: Kind::Hooks,
    }));
    parts
}

/// The model and token usage of the Copilot session a `Stop` hook reports on, read from
/// the agent's own event log into `report`.
///
/// Recent VS Code runs Copilot on the Copilot CLI agent, which logs each model call as a
/// `session.usage_record` in `<copilot>/session-state/<session>/events.jsonl`; the hook
/// names that file (`transcriptPath`) and the session. Tokens add up over the session;
/// Copilot's `inputTokens` include the cached ones, which are counted apart, as OpenCode
/// does. Its cost is in premium requests, not dollars, so it is left out. `false` when
/// there is no such log, as with VS Code's older built-in chat.
pub fn read_session(env: &Env, input: &Map<String, Value>, report: &mut Active) -> bool {
    let session = input
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let log = match input.get("transcriptPath").and_then(Value::as_str) {
        Some(path) => PathBuf::from(path),
        None if !session.is_empty() => copilot_dir(env)
            .join("session-state")
            .join(session)
            .join("events.jsonl"),
        None => return false,
    };
    let Ok(text) = fs::read_to_string(&log) else {
        return false;
    };
    let mut found = false;
    for usage in text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| event.get("type").and_then(Value::as_str) == Some("session.usage_record"))
        .filter_map(|event| event.get("data")?.get("usage").cloned())
    {
        let count = |key: &str| {
            usage
                .get(key)
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                .max(0.0) as u64
        };
        let (cache_read, cache_write) = (count("cacheReadTokens"), count("cacheWriteTokens"));
        report.input += count("inputTokens").saturating_sub(cache_read + cache_write);
        report.output += count("outputTokens");
        report.reasoning += count("reasoningTokens");
        report.cache_read += cache_read;
        report.cache_write += cache_write;
        if let Some(model) = ["modelDisplayName", "model"].iter().find_map(|key| {
            usage
                .get(*key)
                .and_then(Value::as_str)
                .filter(|m| !m.is_empty())
        }) {
            report.model = model.to_string();
        }
        found = true;
    }
    if found {
        report.session = session.to_string();
    }
    found
}

/// The model the last Copilot request used, read from VS Code's local chat store.
///
/// Copilot's hooks carry neither the model nor the tokens, so the gauge takes the model
/// here: VS Code keeps one storage folder per workspace (named after its path), and that
/// folder's chat sessions record the model of each request. `None` when nothing matches,
/// and the gauge then shows the client alone.
pub fn current_model(env: &Env, cwd: &str) -> Option<String> {
    if cwd.is_empty() {
        return None;
    }
    let storage = workspace_storage(env, cwd)?;
    let session = newest_file(&storage.join("chatSessions"))?;
    let text = fs::read_to_string(&session).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    value
        .get("requests")?
        .as_array()?
        .iter()
        .rev()
        .find_map(|request| request.get("modelId").and_then(Value::as_str))
        .filter(|model| !model.is_empty())
        .map(str::to_string)
}

/// The `workspaceStorage/<hash>` folder of the workspace `cwd`, found through the folder
/// path each storage keeps in its `workspace.json`.
fn workspace_storage(env: &Env, cwd: &str) -> Option<PathBuf> {
    let wanted = cwd.trim_end_matches(['/', '\\']).to_string();
    fs::read_dir(user_dir(env).join("workspaceStorage"))
        .ok()?
        .flatten()
        .find(|entry| {
            fs::read_to_string(entry.path().join("workspace.json"))
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .and_then(|value| value.get("folder")?.as_str().map(str::to_string))
                .is_some_and(|folder| uri_to_path(&folder) == wanted)
        })
        .map(|entry| entry.path())
}

/// `file:///a/b%20c` to `/a/b c`.
fn uri_to_path(uri: &str) -> String {
    let mut bytes: Vec<u8> = Vec::new();
    let mut chars = uri
        .strip_prefix("file://")
        .unwrap_or(uri)
        .chars()
        .peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            match u8::from_str_radix(&hex, 16) {
                Ok(byte) => bytes.push(byte),
                Err(_) => {
                    bytes.push(b'%');
                    bytes.extend_from_slice(hex.as_bytes());
                }
            }
        } else {
            let mut buffer = [0u8; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The most recently modified `.json` file in `dir`.
fn newest_file(dir: &Path) -> Option<PathBuf> {
    fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| {
            let modified: SystemTime = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.path()))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}

fn render(subagent: &Subagent) -> String {
    let mut header = format!(
        "name: {}\ndescription: {}\n",
        subagent.name,
        yaml_string(subagent.description)
    );
    if let Some(model) = model(subagent.tier) {
        header += &format!("model: {}\n", yaml_string(model));
    }
    if subagent.read_only {
        header += "tools: ['read', 'search']\n";
    }
    format!("---\n{header}---\n\n{}", subagents::body(subagent))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_model_comes_from_the_workspace_chat_store() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env {
            home: dir.path().to_path_buf(),
            ..Env::default()
        };
        let storage = user_dir(&env).join("workspaceStorage").join("hash");
        fs::create_dir_all(storage.join("chatSessions")).unwrap();
        fs::write(
            storage.join("workspace.json"),
            json!({ "folder": "file:///work/site" }).to_string(),
        )
        .unwrap();
        fs::write(
            storage.join("chatSessions").join("session.json"),
            json!({ "requests": [
                { "modelId": "gpt-4o" },
                { "modelId": "claude-sonnet-4.5" }
            ] })
            .to_string(),
        )
        .unwrap();

        assert_eq!(
            current_model(&env, "/work/site").as_deref(),
            Some("claude-sonnet-4.5"),
            "the model of the last request"
        );
        assert_eq!(current_model(&env, "/elsewhere"), None);
        assert_eq!(current_model(&env, ""), None);
    }

    #[test]
    fn a_file_uri_becomes_a_path() {
        assert_eq!(uri_to_path("file:///Users/a%20b/site"), "/Users/a b/site");
        assert_eq!(uri_to_path("/plain/path"), "/plain/path");
    }
}
