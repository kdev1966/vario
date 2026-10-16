//! Cursor: sub-agents in `~/.cursor/agents/*.md`, for every project.
//!
//! Cursor also reads `~/.claude/agents/`, where Claude Code's sub-agents name a
//! Claude model alias; a file of the same name in `.cursor/agents/` takes precedence,
//! so each Vario sub-agent gets a Cursor twin. Cursor has no instructions file shared
//! by all projects (its User Rules live in its settings), so the sub-agents'
//! descriptions alone prompt the delegation.

use std::path::PathBuf;

use super::subagents::{self, MARKER, Subagent, Tier, yaml_string};
use super::{Env, Integration, has_mcp_server};
use crate::handoff;
use crate::parts::{Kind, OwnFile, Part};

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
    subagents::all()
        .iter()
        .map(|subagent| -> Box<dyn Part> {
            Box::new(OwnFile {
                path: dir.join(format!("{}.md", subagent.name)),
                content: render(subagent),
                kind: Kind::Subagents,
                marker: Some(MARKER.into()),
            })
        })
        .collect()
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
