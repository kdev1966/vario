//! Cursor: sub-agents in `~/.cursor/agents/*.md` and hooks in `~/.cursor/hooks.json`,
//! for every project.
//!
//! Cursor also reads `~/.claude/agents/`, where Claude Code's sub-agents name a
//! Claude model alias; a file of the same name in `.cursor/agents/` takes precedence,
//! so each Vario sub-agent gets a Cursor twin. Cursor has no instructions file shared
//! by all projects (its User Rules live in its settings), so a `sessionStart` hook
//! injects the delegation guidance; `beforeSubmitPrompt` updates the active client and
//! `subagentStop` journals each delegation.

use std::path::PathBuf;

use serde_json::{Map, Value, json};

use super::subagents::{self, MARKER, Subagent, Tier, yaml_string};
use super::{Env, Integration, has_mcp_server};
use crate::handoff;
use crate::parts::{JsonHooks, Kind, OwnFile, Part};

pub fn integration() -> Integration {
    Integration {
        name: "cursor",
        label: "Cursor",
        detect: |env| env.which("cursor").is_some() || config_dir(env).is_dir(),
        parts,
        handoff_connected: |env| {
            has_mcp_server(
                &config_dir(env).join("mcp.json"),
                "mcpServers",
                handoff::SERVER_NAME,
            )
        },
    }
}

fn config_dir(env: &Env) -> PathBuf {
    env.home.join(".cursor")
}

/// Cursor bills a named third-party model at its list price, and on the older
/// request-based plans sub-agents run on Composer whatever they ask for. Cursor's
/// own Composer therefore stays economical on every plan, without detecting it.
/// `fast=false` selects the standard variant, six times cheaper than the fast one.
/// Until the tier-to-model mapping is settled (design document, open questions),
/// both tiers use it.
fn model(tier: Tier) -> &'static str {
    match tier {
        Tier::Economical | Tier::Intermediate => "composer-2.5[fast=false]",
    }
}

fn parts(env: &Env) -> Vec<Box<dyn Part>> {
    let dir = config_dir(env).join("agents");
    let mut parts: Vec<Box<dyn Part>> = subagents::all()
        .iter()
        .map(|subagent| -> Box<dyn Part> {
            Box::new(OwnFile {
                path: dir.join(format!("{}.md", subagent.name)),
                content: render(subagent),
                kind: Kind::Subagents,
                marker: Some(MARKER.into()),
            })
        })
        .collect();
    // The hook that injects the guidance plays the instructions' role.
    parts.push(hook(
        env,
        "sessionStart",
        "session-start",
        Kind::Instructions,
    ));
    parts.push(hook(env, "subagentStop", "subagent-stop", Kind::Hooks));
    parts.push(hook(env, "beforeSubmitPrompt", "activity", Kind::Hooks));
    parts
}

fn hook(env: &Env, event: &str, vario_event: &str, kind: Kind) -> Box<dyn Part> {
    let mut defaults = Map::new();
    defaults.insert("version".into(), Value::from(1));
    Box::new(JsonHooks {
        path: config_dir(env).join("hooks.json"),
        keys: vec!["hooks".into()],
        entries: vec![(
            event.into(),
            json!({
                "command": env.hook_command(vario_event, "cursor"),
                "timeout": 10,
            }),
        )],
        defaults,
        kind,
    })
}

fn render(subagent: &Subagent) -> String {
    format!(
        "---\nname: {}\ndescription: {}\nmodel: {}\nreadonly: {}\n---\n\n{}",
        subagent.name,
        yaml_string(subagent.description),
        yaml_string(model(subagent.tier)),
        subagent.read_only,
        subagents::body(subagent),
    )
}
