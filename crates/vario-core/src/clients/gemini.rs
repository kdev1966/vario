//! Google Gemini CLI: sub-agents in `<home>/agents/*.md` (YAML header) and the
//! delegation guidance in `<home>/GEMINI.md`, for every project. `<home>` is
//! `$GEMINI_CLI_HOME/.gemini`, or `~/.gemini`.
//!
//! Gemini CLI delegates on its own from the sub-agents' descriptions. It has no event
//! when a sub-agent ends, so its delegations are not journaled. An `AfterAgent` hook in
//! `<home>/settings.json` tells the gauge, after each turn, the model and tokens in use.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde_json::{Map, Value, json};

use super::subagents::{self, MARKER, Subagent, Tier, yaml_string};
use super::{Env, Integration, has_mcp_server};
use crate::active::Active;
use crate::handoff;
use crate::parts::{JsonHooks, Kind, MarkdownBlock, OwnFile, Part};

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
    parts.push(Box::new(JsonHooks {
        path: dir.join("settings.json"),
        keys: vec!["hooks".into()],
        entries: vec![(
            "AfterAgent".into(),
            json!({
                "matcher": "*",
                "hooks": [{
                    "type": "command",
                    "command": env.hook_command("activity", "gemini"),
                    "name": "vario",
                    // Gemini CLI counts in milliseconds.
                    "timeout": 5000,
                }],
            }),
        )],
        defaults: Map::new(),
        kind: Kind::Hooks,
    }));
    parts
}

/// The model and token usage of the Gemini CLI session an `AfterAgent` hook reports on,
/// read from its transcript (`transcript_path`) into `report`.
///
/// The transcript is a JSON Lines log Gemini CLI replays: each model reply is a
/// `"type": "gemini"` message with its `model` and `tokens`, written on a line of its
/// own or listed again in a `$set` record; the last copy of an `id` wins. Messages a
/// rewind drops still count, since their tokens were spent. Tokens add up over the session; Gemini's `input` includes the
/// `cached` tokens, which are counted apart, as OpenCode does. Gemini CLI reports no
/// cost. `false` when there is no transcript.
pub fn read_session(input: &Map<String, Value>, report: &mut Active) -> bool {
    let Some(log) = input
        .get("transcript_path")
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty())
    else {
        return false;
    };
    let Ok(text) = fs::read_to_string(log) else {
        return false;
    };
    let mut replies: Vec<Value> = Vec::new();
    let mut by_id: HashMap<String, usize> = HashMap::new();
    // A message is a line of its own, or one of the `messages` a `$set` record (or an
    // older whole-conversation record) lists anew.
    let messages = text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .flat_map(|record| {
            let listed = record
                .get("$set")
                .unwrap_or(&record)
                .get("messages")
                .and_then(Value::as_array)
                .cloned();
            listed.unwrap_or_else(|| vec![record])
        })
        .filter(|message| message.get("type").and_then(Value::as_str) == Some("gemini"));
    for message in messages {
        let id = message
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string);
        match id {
            Some(id) if by_id.contains_key(&id) => replies[by_id[&id]] = message,
            Some(id) => {
                by_id.insert(id, replies.len());
                replies.push(message);
            }
            None => replies.push(message),
        }
    }
    for reply in &replies {
        if let Some(model) = reply
            .get("model")
            .and_then(Value::as_str)
            .filter(|model| !model.is_empty())
        {
            report.model = model.to_string();
        }
        let Some(tokens) = reply.get("tokens") else {
            continue;
        };
        let count = |key: &str| {
            tokens
                .get(key)
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                .max(0.0) as u64
        };
        let cached = count("cached");
        report.input += count("input").saturating_sub(cached);
        report.output += count("output");
        report.reasoning += count("thoughts");
        report.cache_read += cached;
    }
    if !replies.is_empty() {
        report.session = input
            .get("session_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
    }
    !replies.is_empty()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_adds_up_each_reply_once() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("session.jsonl");
        let reply = |id: &str, input: u64, output: u64| {
            json!({"id": id, "type": "gemini", "model": "gemini-3-flash",
                   "tokens": {"input": input, "output": output, "cached": 100,
                              "thoughts": 7, "tool": 0, "total": input + output}})
        };
        let user = json!({"id": "u1", "type": "user", "content": "hi"});
        let lines = [
            json!({"sessionId": "s1", "projectHash": "h", "kind": "main"}),
            json!({"$set": {"messages": [user.clone()]}}),
            reply("g1", 1_000, 10),
            // The same reply, written again once its tokens are known.
            reply("g1", 1_100, 20),
            json!({"$set": {"messages": [user, reply("g1", 1_100, 20), reply("g2", 600, 5)]}}),
            json!({"$rewindTo": "g2"}),
        ];
        let text: Vec<String> = lines.iter().map(Value::to_string).collect();
        fs::write(&log, text.join("\n") + "\nnot json\n").unwrap();
        let input = json!({"session_id": "s1", "transcript_path": log})
            .as_object()
            .unwrap()
            .clone();
        let mut report = Active::default();
        assert!(read_session(&input, &mut report));
        assert_eq!(report.model, "gemini-3-flash");
        assert_eq!(report.input, 1_000 + 500, "cached tokens are counted apart");
        assert_eq!(report.cache_read, 200);
        assert_eq!(report.output, 25);
        assert_eq!(report.reasoning, 14);
        assert_eq!(report.session, "s1");
    }

    #[test]
    fn no_transcript_reads_nothing() {
        let mut report = Active::default();
        let input = json!({"transcript_path": ""}).as_object().unwrap().clone();
        assert!(!read_session(&input, &mut report));
        assert_eq!(report, Active::default());
    }
}
