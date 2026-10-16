//! What `vario hook <event> --via <client>` does when a client runs it.
//!
//! Hooks run on every session or sub-agent, so they must be fast and never get in the
//! way: any failure is written to `<data>/hooks.log` and the output stays valid for the
//! client.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use serde_json::{Map, Value, json};

use crate::clients::subagents::{self, SESSION_CONTEXT};
use crate::clients::{Env, by_name};
use crate::install;
use crate::lang::Lang;
use crate::parts::Kind;
use crate::quota::{self, Level, Quota};
use crate::tr;

/// One JSON line per delegation to a Vario sub-agent.
pub const JOURNAL: &str = "delegations.jsonl";
const LOG: &str = "hooks.log";
/// The log is emptied when it grows past this size.
const LOG_LIMIT: u64 = 1 << 20;
/// Longest text kept from a footer field.
const FIELD_LIMIT: usize = 300;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    SessionStart,
    SubagentStop,
    /// Before each prompt: the thermostat's advice, from the quota.
    PromptSubmit,
}

impl Event {
    pub fn parse(name: &str) -> Option<Event> {
        match name {
            "session-start" => Some(Event::SessionStart),
            "subagent-stop" => Some(Event::SubagentStop),
            "prompt-submit" => Some(Event::PromptSubmit),
            _ => None,
        }
    }
}

/// The client whose configuration holds the hook.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    Claude,
    Cursor,
    /// Vario's OpenCode plugin, which passes a sub-agent's end in Cursor's shape.
    OpenCode,
}

impl Via {
    pub fn parse(name: &str) -> Option<Via> {
        match name {
            "claude" => Some(Via::Claude),
            "cursor" => Some(Via::Cursor),
            "opencode" => Some(Via::OpenCode),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Via::Claude => "claude",
            Via::Cursor => "cursor",
            Via::OpenCode => "opencode",
        }
    }
}

/// Handle one hook call; returns what to print on standard output.
pub fn run(event: Event, via: Via, input: &str, env: &Env, data_dir: &Path) -> String {
    let input = match serde_json::from_str(input) {
        Ok(Value::Object(map)) => map,
        _ => {
            if !input.trim().is_empty() {
                log(data_dir, "the hook input is not a JSON object; ignored");
            }
            Map::new()
        }
    };
    let in_cursor = env.var("CURSOR_VERSION").is_some() || input.contains_key("cursor_version");
    // Cursor also runs Claude Code's hooks. When Vario's own Cursor hooks are installed,
    // they do the work and the Claude Code copy stands aside, so nothing runs twice.
    if via == Via::Claude && in_cursor && cursor_hooks_installed(env) {
        return quiet(via);
    }
    match event {
        Event::SessionStart => match via {
            Via::Cursor | Via::OpenCode => {
                json!({ "additional_context": SESSION_CONTEXT }).to_string()
            }
            Via::Claude => json!({
                "hookSpecificOutput": {
                    "hookEventName": "SessionStart",
                    "additionalContext": SESSION_CONTEXT,
                }
            })
            .to_string(),
        },
        // The quota is Claude Code's subscription; inside Cursor it says nothing.
        Event::PromptSubmit if in_cursor => quiet(via),
        Event::PromptSubmit => thermostat(env, data_dir).unwrap_or_else(|| quiet(via)),
        Event::SubagentStop => {
            let client = if in_cursor { "cursor" } else { via.name() };
            if let Err(err) = record(&input, client, &env.home, data_dir) {
                log(data_dir, &format!("subagent-stop: {err}"));
            }
            quiet(via)
        }
    }
}

/// The thermostat's advice for this prompt, in Claude Code's `UserPromptSubmit`
/// output: guidance for the model, and above the alert threshold a message for the
/// user. `None` under the first threshold or without a current quota.
fn thermostat(env: &Env, data_dir: &Path) -> Option<String> {
    let now = quota::now();
    let quota = Quota::load(data_dir)?;
    let window = quota.highest(now)?;
    let used = window.used_percentage.round();
    let left = quota::time_left(window.resets_at, now);
    let lang = Lang::detect(env);
    let (context, message) = match quota.level(now) {
        Level::Normal => return None,
        Level::Reinforced => (
            format!(
                "Vario: the subscription quota is at {used}%. Delegate all reading and \
                 summarizing to vario-reader and every bounded task with a check to \
                 vario-worker; keep only decisions and review in this session."
            ),
            None,
        ),
        Level::Alert => (
            format!(
                "Vario: the subscription quota is at {used}%, resetting in {left}. Delegate \
                 everything an economical sub-agent can do and avoid costly exploration in \
                 this session."
            ),
            Some(tr!(
                lang,
                "Vario: quota at {used}%, resets in {left}. Delegation is reinforced.",
                "Vario : quota à {used} %, remise à zéro dans {left}. Délégation renforcée."
            )),
        ),
        Level::Limit => (
            format!(
                "Vario: the subscription quota is used up; it resets in {left}. Tell the user \
                 they can switch to another agent, and save a handoff first (Handoff's \
                 memory_save) so the next agent can continue."
            ),
            Some(tr!(
                lang,
                "Vario: quota used up, resets in {left}. Consider switching agents; save a handoff first.",
                "Vario : quota épuisé, remise à zéro dans {left}. Pensez à changer d'agent, après avoir enregistré une passation."
            )),
        ),
    };
    let mut output = json!({
        "hookSpecificOutput": {
            "hookEventName": "UserPromptSubmit",
            "additionalContext": context,
        }
    });
    if let Some(message) = message {
        output["systemMessage"] = Value::from(message);
    }
    Some(output.to_string())
}

/// An output that changes nothing: Cursor expects JSON, the others accept nothing.
fn quiet(via: Via) -> String {
    match via {
        Via::Cursor => "{}".into(),
        Via::Claude | Via::OpenCode => String::new(),
    }
}

fn cursor_hooks_installed(env: &Env) -> bool {
    by_name("cursor")
        .is_some_and(|cursor| install::status(&cursor, env).contains(&(Kind::Hooks, true)))
}

/// Append the delegation to the journal, if the sub-agent is one of Vario's. Claude
/// Code names it `agent_type` and passes its last message; Cursor names it
/// `subagent_type` and passes a summary.
fn record(
    input: &Map<String, Value>,
    client: &str,
    home: &Path,
    data_dir: &Path,
) -> Result<(), String> {
    let text = |key: &str| input.get(key).and_then(Value::as_str);
    let Some(agent) = text("agent_type").or_else(|| text("subagent_type")) else {
        return Ok(());
    };
    if !subagents::is_ours(agent) {
        return Ok(());
    }
    let mut footer = parse_footer(
        text("last_assistant_message")
            .or_else(|| text("summary"))
            .unwrap_or_default(),
    );
    // Some hosts have the sub-agent hand its report back through a tool call, so
    // the last message holds no footer: read it from the sub-agent's transcript.
    if footer.result.is_none()
        && let Some(path) = text("agent_transcript_path")
        && let Some(report) = last_report(&expand_home(path, home))
    {
        footer = parse_footer(&report);
    }
    let project = text("cwd").map(str::to_owned).or_else(|| {
        input
            .get("workspace_roots")
            .and_then(Value::as_array)
            .and_then(|roots| roots.first())
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    let entry = json!({
        "time": chrono::Local::now().to_rfc3339(),
        "client": client,
        "agent": agent,
        "status": input.get("status"),
        "result": footer.result,
        "checks": footer.checks,
        "uncertainties": footer.uncertainties,
        "escalate": footer.escalate,
        "reason": footer.reason,
        "duration_ms": input.get("duration_ms"),
        "model": input.get("subagent_model"),
        "cost": input.get("cost"),
        "tokens": input.get("tokens"),
        "project": project,
    });
    append(&data_dir.join(JOURNAL), &format!("{entry}\n"))
}

/// The report footer every Vario sub-agent ends with, in English or French.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Footer {
    pub result: Option<String>,
    pub checks: Option<String>,
    pub uncertainties: Option<String>,
    /// `None` when the answer is neither yes nor no.
    pub escalate: Option<bool>,
    pub reason: Option<String>,
}

/// Read the footer's fields; the last occurrence of each wins, since the footer ends
/// the answer.
pub fn parse_footer(text: &str) -> Footer {
    let mut footer = Footer::default();
    for line in text.lines() {
        let line = line.trim().trim_start_matches(['-', '*', '>', ' ']);
        let Some((label, value)) = line.split_once(':') else {
            continue;
        };
        let label = label.trim().trim_matches('*').trim().to_uppercase();
        let value = value.trim().trim_matches('*').trim();
        match label.as_str() {
            "RESULT" | "RÉSULTAT" | "RESULTAT" => footer.result = Some(clip(value)),
            "CHECKS" | "VÉRIFICATIONS" | "VERIFICATIONS" => footer.checks = Some(clip(value)),
            "UNCERTAINTIES" | "INCERTITUDES" => footer.uncertainties = Some(clip(value)),
            "ESCALATE" | "ESCALADER" => {
                let (answer, reason) = match value.split_once(['-', '–', '—']) {
                    Some((answer, reason)) => (answer, Some(reason)),
                    None => (value, None),
                };
                footer.escalate = match answer.trim().to_lowercase().as_str() {
                    "yes" | "oui" => Some(true),
                    "no" | "non" => Some(false),
                    _ => None,
                };
                footer.reason = reason
                    .map(str::trim)
                    .filter(|reason| !reason.is_empty())
                    .map(clip);
            }
            _ => {}
        }
    }
    footer
}

/// The last assistant entry of a Claude Code transcript (JSON lines): its texts and
/// the string inputs of its tool calls, joined.
fn last_report(transcript: &Path) -> Option<String> {
    const TAIL: u64 = 4 << 20;
    let bytes = fs::read(transcript).ok()?;
    let start = bytes.len().saturating_sub(TAIL as usize);
    let text = String::from_utf8_lossy(&bytes[start..]);
    text.lines().rev().find_map(|line| {
        let entry: Value = serde_json::from_str(line).ok()?;
        if entry.get("type").and_then(Value::as_str) != Some("assistant") {
            return None;
        }
        let parts = entry.pointer("/message/content")?.as_array()?;
        let mut pieces = Vec::new();
        for part in parts {
            match part.get("type").and_then(Value::as_str) {
                Some("text") => pieces.extend(part.get("text").and_then(Value::as_str)),
                Some("tool_use") => {
                    if let Some(Value::Object(input)) = part.get("input") {
                        pieces.extend(input.values().filter_map(Value::as_str));
                    }
                }
                _ => {}
            }
        }
        (!pieces.is_empty()).then(|| pieces.join("\n"))
    })
}

/// `~/x` relative to `home`; Claude Code's examples show transcript paths that way.
fn expand_home(path: &str, home: &Path) -> std::path::PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => Path::new(path).to_path_buf(),
    }
}

fn clip(text: &str) -> String {
    if text.chars().count() <= FIELD_LIMIT {
        return text.to_string();
    }
    let kept: String = text.chars().take(FIELD_LIMIT - 1).collect();
    kept + "…"
}

fn append(path: &Path, line: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("{}: {err}", parent.display()))?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| file.write_all(line.as_bytes()))
        .map_err(|err| format!("{}: {err}", path.display()))
}

/// Note a failure where the user can find it, without ever failing the hook.
pub(crate) fn log(data_dir: &Path, message: &str) {
    let path = data_dir.join(LOG);
    if fs::metadata(&path).is_ok_and(|meta| meta.len() > LOG_LIMIT) {
        let _ = fs::remove_file(&path);
    }
    let line = format!("{} {message}\n", chrono::Local::now().to_rfc3339());
    let _ = append(&path, &line);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_footer_in_a_code_block() {
        let text = "Done.\n\n```\nRESULT: renamed 3 files\nCHECKS: cargo test (ok)\n\
                    UNCERTAINTIES: none\nESCALATE: no - nothing to decide\n```";
        assert_eq!(
            parse_footer(text),
            Footer {
                result: Some("renamed 3 files".into()),
                checks: Some("cargo test (ok)".into()),
                uncertainties: Some("none".into()),
                escalate: Some(false),
                reason: Some("nothing to decide".into()),
            }
        );
    }

    #[test]
    fn french_footer_with_markdown_and_a_bare_answer() {
        let text = "**RÉSULTAT :** résumé fait\n- VÉRIFICATIONS : aucune\nESCALADER : oui";
        let footer = parse_footer(text);
        assert_eq!(footer.result.as_deref(), Some("résumé fait"));
        assert_eq!(footer.checks.as_deref(), Some("aucune"));
        assert_eq!(footer.escalate, Some(true));
        assert_eq!(footer.reason, None);
    }

    #[test]
    fn missing_footer_and_long_fields() {
        assert_eq!(parse_footer("no footer here"), Footer::default());
        let long = format!("RESULT: {}", "x".repeat(500));
        let result = parse_footer(&long).result.unwrap();
        assert_eq!(result.chars().count(), FIELD_LIMIT);
        assert!(result.ends_with('…'));
    }

    fn env_in(dir: &Path) -> Env {
        Env {
            home: dir.join("home"),
            ..Env::default()
        }
    }

    fn journal(data: &Path) -> Vec<Value> {
        fs::read_to_string(data.join(JOURNAL))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn claude_code_subagent_stop_is_journaled() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let input = json!({
            "hook_event_name": "SubagentStop",
            "cwd": "/work/vario",
            "agent_type": "vario-worker",
            "last_assistant_message": "RESULT: ok\nCHECKS: cargo test\nESCALATE: no - fine",
        });
        let out = run(
            Event::SubagentStop,
            Via::Claude,
            &input.to_string(),
            &env_in(dir.path()),
            &data,
        );
        assert_eq!(out, "");
        let entries = journal(&data);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["client"], "claude");
        assert_eq!(entries[0]["agent"], "vario-worker");
        assert_eq!(entries[0]["escalate"], false);
        assert_eq!(entries[0]["project"], "/work/vario");
    }

    #[test]
    fn a_report_handed_back_through_a_tool_is_read_from_the_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let env = env_in(dir.path());
        let transcript = env.home.join("agent.jsonl");
        fs::create_dir_all(&env.home).unwrap();
        let entries = [
            json!({"type": "user", "message": {"role": "user", "content": "GOAL: …"}}),
            json!({"type": "assistant", "message": {"content": [
                {"type": "text", "text": "Reading the types."},
                {"type": "tool_use", "name": "Read", "input": {"file_path": "/x"}},
            ]}}),
            json!({"type": "assistant", "message": {"content": [
                {"type": "tool_use", "name": "SubagentHandback", "input": {
                    "message": "Done.\nRÉSULTAT : API décrite\nESCALADER : oui - à confirmer"
                }},
            ]}}),
            json!({"type": "user", "message": {"content": [{"type": "tool_result"}]}}),
        ];
        let lines: Vec<String> = entries.iter().map(Value::to_string).collect();
        fs::write(&transcript, lines.join("\n")).unwrap();
        let input = json!({
            "agent_type": "vario-reader",
            "last_assistant_message": "",
            "agent_transcript_path": "~/agent.jsonl",
        });
        run(
            Event::SubagentStop,
            Via::Claude,
            &input.to_string(),
            &env,
            &data,
        );
        let entries = journal(&data);
        assert_eq!(entries[0]["result"], "API décrite");
        assert_eq!(entries[0]["escalate"], true);
        assert_eq!(entries[0]["reason"], "à confirmer");
    }

    #[test]
    fn other_sub_agents_and_bad_input_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let env = env_in(dir.path());
        let explore = json!({"agent_type": "Explore", "last_assistant_message": "RESULT: x"});
        run(
            Event::SubagentStop,
            Via::Claude,
            &explore.to_string(),
            &env,
            &data,
        );
        assert_eq!(
            run(Event::SubagentStop, Via::Cursor, "not json", &env, &data),
            "{}"
        );
        assert!(journal(&data).is_empty());
    }

    #[test]
    fn cursor_session_start_injects_the_guidance() {
        let dir = tempfile::tempdir().unwrap();
        let out = run(
            Event::SessionStart,
            Via::Cursor,
            "{}",
            &env_in(dir.path()),
            dir.path(),
        );
        let value: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value["additional_context"], SESSION_CONTEXT);
    }

    #[test]
    fn in_cursor_the_claude_code_copy_stands_aside_only_when_cursor_hooks_exist() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let env = env_in(dir.path());
        let input = json!({
            "cursor_version": "3.1.0",
            "subagent_type": "vario-reader",
            "workspace_roots": ["/work/site"],
            "summary": "RESULT: listed\nESCALATE: no",
        })
        .to_string();

        // Vario installed for Claude Code only: the Claude Code copy records, as Cursor.
        run(Event::SubagentStop, Via::Claude, &input, &env, &data);
        let entries = journal(&data);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["client"], "cursor");
        assert_eq!(entries[0]["project"], "/work/site");

        // Vario's Cursor hooks installed: only the native one records.
        let cursor = by_name("cursor").unwrap();
        let mut session = crate::files::Session::with_backup_dir(false, dir.path().join("b"));
        install::apply(&mut session, &[cursor], &env);
        run(Event::SubagentStop, Via::Claude, &input, &env, &data);
        assert_eq!(journal(&data).len(), 1, "the Claude Code copy stood aside");
        run(Event::SubagentStop, Via::Cursor, &input, &env, &data);
        assert_eq!(journal(&data).len(), 2);
    }
}
