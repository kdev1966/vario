//! Claude Code: sub-agents in `<config>/agents/*.md` (YAML header with a `model`
//! alias), the delegation guidance in `<config>/CLAUDE.md`, and in
//! `<config>/settings.json` a `SubagentStop` hook that journals each delegation, a
//! `SessionStart` hook that opens the floating gauge, a `UserPromptSubmit` hook for
//! the quota thermostat, and the status line that captures the quota, for every
//! project. `<config>` is `$CLAUDE_CONFIG_DIR`, or
//! `~/.claude`.

use std::path::PathBuf;

use serde_json::{Map, json};

use super::subagents::{self, MARKER, Subagent, Tier, yaml_string};
use super::{Env, Integration, has_mcp_server};
use crate::handoff;
use crate::parts::{JsonHooks, Kind, MarkdownBlock, OwnFile, Part, StatusLine};

pub fn integration() -> Integration {
    Integration {
        name: "claude",
        label: "Claude Code",
        detect: |env| env.which("claude").is_some() || config_dir(env).is_dir(),
        parts,
        handoff_connected,
    }
}

pub(crate) fn config_dir(env: &Env) -> PathBuf {
    env.var("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| env.home.join(".claude"))
}

/// User-scope MCP servers live in `.claude.json`: inside `$CLAUDE_CONFIG_DIR` when
/// it is set, in the home directory otherwise.
pub(crate) fn user_state_file(env: &Env) -> PathBuf {
    match env.var("CLAUDE_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir).join(".claude.json"),
        None => env.home.join(".claude.json"),
    }
}

fn handoff_connected(env: &Env) -> bool {
    has_mcp_server(&user_state_file(env), "mcpServers", handoff::SERVER_NAME)
}

/// Claude Code's model aliases follow each family's latest version. Until the
/// tier-to-model mapping is settled (design document, open questions), these are
/// the defaults.
fn model(tier: Tier) -> &'static str {
    match tier {
        Tier::Economical => "haiku",
        Tier::Intermediate => "sonnet",
    }
}

fn parts(env: &Env) -> Vec<Box<dyn Part>> {
    let dir = config_dir(env);
    let mut parts: Vec<Box<dyn Part>> = subagents::all()
        .iter()
        .map(|subagent| -> Box<dyn Part> {
            Box::new(OwnFile {
                path: dir.join("agents").join(format!("{}.md", subagent.name)),
                content: render(subagent),
                kind: Kind::Subagents,
                marker: Some(MARKER.into()),
            })
        })
        .collect();
    parts.push(Box::new(MarkdownBlock {
        path: dir.join("CLAUDE.md"),
        body: subagents::INSTRUCTIONS.into(),
    }));
    parts.push(Box::new(JsonHooks {
        path: dir.join("settings.json"),
        keys: vec!["hooks".into()],
        entries: vec![
            (
                "SubagentStop".into(),
                json!({
                    "matcher": subagents::matcher(),
                    "hooks": [{
                        "type": "command",
                        "command": env.hook_command("subagent-stop", "claude"),
                        "timeout": 10,
                    }],
                }),
            ),
            (
                "SessionStart".into(),
                json!({
                    "matcher": "startup|resume",
                    "hooks": [{
                        "type": "command",
                        "command": env.hook_command("session-start", "claude"),
                        "timeout": 5,
                    }],
                }),
            ),
            (
                "UserPromptSubmit".into(),
                json!({
                    "hooks": [{
                        "type": "command",
                        "command": env.hook_command("prompt-submit", "claude"),
                        "timeout": 5,
                    }],
                }),
            ),
        ],
        defaults: Map::new(),
        kind: Kind::Hooks,
    }));
    parts.push(Box::new(StatusLine {
        path: dir.join("settings.json"),
        command: env.status_line_command(),
        kind: Kind::Quota,
    }));
    parts
}

fn render(subagent: &Subagent) -> String {
    let tools = if subagent.read_only {
        "Read, Grep, Glob"
    } else {
        "Read, Grep, Glob, Edit, Write, Bash"
    };
    format!(
        "---\nname: {}\ndescription: {}\ntools: {tools}\nmodel: {}\n---\n\n{}",
        subagent.name,
        yaml_string(subagent.description),
        model(subagent.tier),
        subagents::body(subagent),
    )
}
