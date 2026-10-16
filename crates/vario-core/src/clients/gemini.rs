//! Google Gemini CLI: sub-agents in `<home>/agents/*.md` (YAML header) and the
//! delegation guidance in `<home>/GEMINI.md`, for every project. `<home>` is
//! `$GEMINI_CLI_HOME/.gemini`, or `~/.gemini`.
//!
//! Gemini CLI delegates on its own from the sub-agents' descriptions. It has no event
//! when a sub-agent ends, so its delegations are not journaled.

use std::path::PathBuf;

use super::subagents::{self, MARKER, Subagent, Tier, yaml_string};
use super::{Env, Integration, has_mcp_server};
use crate::handoff;
use crate::parts::{Kind, MarkdownBlock, OwnFile, Part};

pub fn integration() -> Integration {
    Integration {
        name: "gemini",
        label: "Gemini CLI",
        detect: |env| env.which("gemini").is_some() || config_dir(env).is_dir(),
        parts,
        handoff_connected: |env| {
            has_mcp_server(
                &config_dir(env).join("settings.json"),
                "mcpServers",
                handoff::SERVER_NAME,
            )
        },
    }
}

fn config_dir(env: &Env) -> PathBuf {
    env.var("GEMINI_CLI_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| env.home.clone())
        .join(".gemini")
}

/// The economical tier uses Gemini CLI's `flash-lite` alias; the intermediate tier
/// inherits the session's model.
fn model(tier: Tier) -> &'static str {
    match tier {
        Tier::Economical => "flash-lite",
        Tier::Intermediate => "inherit",
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
        path: dir.join("GEMINI.md"),
        body: subagents::INSTRUCTIONS.into(),
    }));
    parts
}

fn render(subagent: &Subagent) -> String {
    // Without `tools`, a sub-agent gets every tool; the reader gets reading tools only.
    let tools = if subagent.read_only {
        "tools:\n  - read_file\n  - grep_search\n"
    } else {
        ""
    };
    format!(
        "---\nname: {}\ndescription: {}\nmodel: {}\n{tools}---\n\n{}",
        subagent.name,
        yaml_string(subagent.description),
        model(subagent.tier),
        subagents::body(subagent),
    )
}
