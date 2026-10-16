//! OpenCode: sub-agents in `<config>/agents/*.md` (`mode: subagent`, named after the
//! file) and the delegation guidance in the global rules file, for every project.
//! `<config>` is `$XDG_CONFIG_HOME/opencode`, or `~/.config/opencode`, on every system.

use std::path::PathBuf;

use super::subagents::{self, MARKER, Subagent, Tier, yaml_string};
use super::{Env, Integration, has_mcp_server};
use crate::files;
use crate::handoff;
use crate::parts::{Kind, MarkdownBlock, OwnFile, Part};

pub fn integration() -> Integration {
    Integration {
        name: "opencode",
        label: "OpenCode",
        detect: |env| env.which("opencode").is_some() || config_dir(env).is_dir(),
        parts,
        handoff_connected: |env| {
            config_files(env)
                .iter()
                .any(|path| has_mcp_server(path, "mcp", handoff::SERVER_NAME))
        },
    }
}

fn config_dir(env: &Env) -> PathBuf {
    env.var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| env.home.join(".config"))
        .join("opencode")
}

/// The global configuration: `$OPENCODE_CONFIG` if set, then `opencode.json` and
/// `opencode.jsonc` in the configuration directory.
fn config_files(env: &Env) -> Vec<PathBuf> {
    let dir = config_dir(env);
    env.var("OPENCODE_CONFIG")
        .map(PathBuf::from)
        .into_iter()
        .chain([dir.join("opencode.json"), dir.join("opencode.jsonc")])
        .collect()
}

/// OpenCode reads the first of `<config>/AGENTS.md` and `~/.claude/CLAUDE.md`, unless
/// its Claude Code fallback is disabled. Creating `AGENTS.md` would silently stop it
/// from reading the user's `CLAUDE.md`, so the guidance goes into the file OpenCode
/// reads now.
fn rules_file(env: &Env) -> PathBuf {
    let agents = config_dir(env).join("AGENTS.md");
    let claude = env.home.join(".claude").join("CLAUDE.md");
    let fallback_disabled = env.var("OPENCODE_DISABLE_CLAUDE_CODE").is_some()
        || env.var("OPENCODE_DISABLE_CLAUDE_CODE_PROMPT").is_some();
    if agents.exists() || fallback_disabled || !claude.is_file() {
        agents
    } else {
        claude
    }
}

/// Models are `provider/model` and depend on the providers the user set up. The
/// economical tier uses the user's `small_model`, which OpenCode keeps for light
/// tasks; without one, and for the intermediate tier, the sub-agent inherits the
/// model of the agent that calls it. A configuration with comments (JSONC) is not
/// read, so its `small_model` is not used.
fn model(tier: Tier, env: &Env) -> Option<String> {
    match tier {
        Tier::Economical => config_files(env).iter().find_map(|path| {
            files::read_json(path)
                .ok()?
                .get("small_model")?
                .as_str()
                .filter(|model| !model.is_empty())
                .map(str::to_owned)
        }),
        Tier::Intermediate => None,
    }
}

fn parts(env: &Env) -> Vec<Box<dyn Part>> {
    let dir = config_dir(env).join("agents");
    let mut parts: Vec<Box<dyn Part>> = subagents::all()
        .iter()
        .map(|subagent| -> Box<dyn Part> {
            Box::new(OwnFile {
                path: dir.join(format!("{}.md", subagent.name)),
                content: render(subagent, model(subagent.tier, env).as_deref()),
                kind: Kind::Subagents,
                marker: Some(MARKER.into()),
            })
        })
        .collect();
    parts.push(Box::new(MarkdownBlock {
        path: rules_file(env),
        body: subagents::INSTRUCTIONS.into(),
    }));
    parts
}

fn render(subagent: &Subagent, model: Option<&str>) -> String {
    let mut header = format!(
        "description: {}\nmode: subagent\n",
        yaml_string(subagent.description)
    );
    if let Some(model) = model {
        header += &format!("model: {}\n", yaml_string(model));
    }
    if subagent.read_only {
        header += "permission:\n  edit: deny\n  bash: deny\n";
    }
    format!("---\n{header}---\n\n{}", subagents::body(subagent))
}
