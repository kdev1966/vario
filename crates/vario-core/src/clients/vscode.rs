//! VS Code with GitHub Copilot: custom agents in `<copilot>/agents/*.agent.md`, the
//! delegation guidance in `<copilot>/copilot-instructions.md` and a `SubagentStop`
//! hook in `<copilot>/hooks/vario.json`, for every workspace. `<copilot>` is
//! `$COPILOT_HOME`, or `~/.copilot`.
//!
//! VS Code delegates through its `agent` tool set (`runSubagent`), on its own or when
//! asked; hooks are a preview that `chat.useHooks` turns on. A sub-agent cannot use a
//! model more expensive than the main one, and models are named as the model picker
//! shows them.

use std::path::PathBuf;

use serde_json::{Map, json};

use super::subagents::{self, MARKER, Subagent, Tier, yaml_string};
use super::{Env, Integration, has_mcp_server};
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
        entries: vec![(
            "SubagentStop".into(),
            json!({
                "type": "command",
                "command": env.hook_command("subagent-stop", "vscode"),
                "timeout": 10,
            }),
        )],
        defaults: Map::new(),
        kind: Kind::Hooks,
    }));
    parts
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
