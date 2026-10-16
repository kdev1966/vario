//! Claude Code: sub-agents in `<config>/agents/*.md` (YAML header with a `model`
//! alias) and the delegation guidance in `<config>/CLAUDE.md`, for every project.
//! `<config>` is `$CLAUDE_CONFIG_DIR`, or `~/.claude`.

use std::path::PathBuf;

use super::subagents::{self, MARKER, Subagent, Tier};
use super::{Env, Integration};
use crate::parts::{Kind, MarkdownBlock, OwnFile, Part};

pub fn integration() -> Integration {
    Integration {
        name: "claude",
        label: "Claude Code",
        detect: |env| env.which("claude").is_some() || config_dir(env).is_dir(),
        parts,
    }
}

fn config_dir(env: &Env) -> PathBuf {
    env.var("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| env.home.join(".claude"))
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

/// A double-quoted YAML scalar, safe whatever the text holds (`: `, `#`, quotes).
fn yaml_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}
