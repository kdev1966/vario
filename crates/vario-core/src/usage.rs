//! Token consumption measured from Claude Code's local transcripts, read only.
//!
//! Claude Code writes one JSON line per content block in
//! `<config>/projects/<project>/<session>.jsonl`, and its sub-agents' in
//! `<session>/subagents/agent-*.jsonl` (older versions mark them `isSidechain` in the
//! session's own file). The lines of one answer share its message id and repeat its
//! usage: each answer is counted once, its content added up across its lines.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::clients::Env;
use crate::clients::claude_code;
use crate::files;

/// Tokens of one model, as the API counts them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    /// Written to the prompt cache (billed above plain input).
    pub cache_write: u64,
    /// Read from the prompt cache (billed far below plain input).
    pub cache_read: u64,
}

impl Tokens {
    fn add(&mut self, other: Tokens) {
        self.input += other.input;
        self.output += other.output;
        self.cache_write += other.cache_write;
        self.cache_read += other.cache_read;
    }

    /// Input, cache writes and output: what the model really processed anew.
    pub fn processed(&self) -> u64 {
        self.input + self.cache_write + self.output
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    /// Unix seconds: the start of the measured period.
    pub since: i64,
    /// Per model, in the main sessions.
    pub main: BTreeMap<String, Tokens>,
    /// Per model, in sub-agents.
    pub delegated: BTreeMap<String, Tokens>,
    /// Tool calls per MCP server (`mcp__<server>__<tool>`).
    pub mcp_calls: BTreeMap<String, u64>,
    /// Answers whose output count was estimated from their length: sub-agents'
    /// transcripts can keep only the count from the start of the answer.
    pub estimated: u64,
}

impl Usage {
    fn total(models: &BTreeMap<String, Tokens>) -> Tokens {
        let mut total = Tokens::default();
        models.values().for_each(|tokens| total.add(*tokens));
        total
    }

    pub fn main_total(&self) -> Tokens {
        Self::total(&self.main)
    }

    pub fn delegated_total(&self) -> Tokens {
        Self::total(&self.delegated)
    }

    /// The share of output tokens written by sub-agents: how much of the work was
    /// delegated. `None` before any answer.
    pub fn delegated_share(&self) -> Option<f64> {
        let delegated = self.delegated_total().output;
        let all = delegated + self.main_total().output;
        (all > 0).then(|| delegated as f64 * 100.0 / all as f64)
    }
}

/// Roughly four characters per token, for answers whose count is clearly incomplete.
const CHARS_PER_TOKEN: u64 = 4;

/// One answer, gathered from its lines.
struct Answer {
    model: String,
    delegated: bool,
    tokens: Tokens,
    content_chars: u64,
}

/// Claude Code's consumption since `since` (Unix seconds).
pub fn claude_code(env: &Env, since: i64) -> Usage {
    let mut usage = Usage {
        since,
        ..Usage::default()
    };
    let cutoff = UNIX_EPOCH + Duration::from_secs(since.max(0) as u64);
    let mut files = Vec::new();
    collect(
        &claude_code::config_dir(env).join("projects"),
        cutoff,
        &mut files,
    );
    let mut answers: HashMap<String, Answer> = HashMap::new();
    let mut anonymous = Vec::new();
    for path in files {
        read(&path, since, &mut answers, &mut anonymous, &mut usage);
    }
    for mut answer in answers.into_values().chain(anonymous) {
        let estimate = answer.content_chars / CHARS_PER_TOKEN;
        if estimate > answer.tokens.output * 2 {
            answer.tokens.output = estimate;
            usage.estimated += 1;
        }
        let models = if answer.delegated {
            &mut usage.delegated
        } else {
            &mut usage.main
        };
        models.entry(answer.model).or_default().add(answer.tokens);
    }
    usage
}

/// The `.jsonl` files under `dir` modified since `cutoff`.
fn collect(dir: &Path, cutoff: SystemTime, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_dir() {
            collect(&path, cutoff, files);
        } else if path.extension().is_some_and(|ext| ext == "jsonl")
            && meta.modified().is_ok_and(|modified| modified >= cutoff)
        {
            files.push(path);
        }
    }
}

fn read(
    path: &Path,
    since: i64,
    answers: &mut HashMap<String, Answer>,
    anonymous: &mut Vec<Answer>,
    usage: &mut Usage,
) {
    let Ok(file) = fs::File::open(path) else {
        return;
    };
    let in_subagents = path
        .components()
        .any(|component| component.as_os_str() == "subagents");
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        // Cheap filter: only assistant messages carry usage.
        if !line.contains("\"assistant\"") || !line.contains("\"usage\"") {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if entry.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let time = entry
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|time| chrono::DateTime::parse_from_rfc3339(time).ok())
            .map(|time| time.timestamp());
        if time.is_none_or(|time| time < since) {
            continue;
        }
        let Some(message) = entry.get("message") else {
            continue;
        };
        let model = message.get("model").and_then(Value::as_str).unwrap_or("?");
        if model.starts_with('<') {
            continue; // "<synthetic>": no model ran
        }
        let count = |key: &str| {
            message
                .pointer(&format!("/usage/{key}"))
                .and_then(Value::as_u64)
                .unwrap_or(0)
        };
        let tokens = Tokens {
            input: count("input_tokens"),
            output: count("output_tokens"),
            cache_write: count("cache_creation_input_tokens"),
            cache_read: count("cache_read_input_tokens"),
        };
        let content = message.get("content");
        let content_chars = content.map_or(0, |content| content.to_string().chars().count() as u64);
        let answer = Answer {
            model: model.to_string(),
            delegated: in_subagents || entry.get("isSidechain") == Some(&Value::Bool(true)),
            tokens,
            content_chars,
        };
        let id = message
            .get("id")
            .or_else(|| entry.get("requestId"))
            .and_then(Value::as_str);
        match id {
            Some(id) => {
                answers
                    .entry(id.to_string())
                    .and_modify(|known| {
                        // Every line repeats the usage; the content adds up.
                        known.tokens.output = known.tokens.output.max(tokens.output);
                        known.content_chars += content_chars;
                    })
                    .or_insert(answer);
            }
            None => anonymous.push(answer),
        }
        for part in content.and_then(Value::as_array).into_iter().flatten() {
            if part.get("type").and_then(Value::as_str) == Some("tool_use")
                && let Some(server) = part
                    .get("name")
                    .and_then(Value::as_str)
                    .and_then(|name| name.strip_prefix("mcp__"))
                    .and_then(|rest| rest.split("__").next())
            {
                *usage.mcp_calls.entry(server.to_string()).or_default() += 1;
            }
        }
    }
}

/// The MCP servers Claude Code connects in every project (`mcpServers` of its user
/// state file).
pub fn claude_code_mcp_servers(env: &Env) -> Vec<String> {
    files::read_json(&claude_code::user_state_file(env))
        .ok()
        .and_then(|data| {
            files::nested(&data, &["mcpServers"]).map(|servers| servers.keys().cloned().collect())
        })
        .unwrap_or_default()
}

/// "950", "12.3 k", "4.1 M".
pub fn human(count: u64) -> String {
    match count {
        0..1_000 => count.to_string(),
        1_000..1_000_000 => format!("{:.1} k", count as f64 / 1e3),
        _ => format!("{:.1} M", count as f64 / 1e6),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn env_in(dir: &Path) -> Env {
        Env {
            home: dir.to_path_buf(),
            ..Env::default()
        }
    }

    fn line(id: &str, model: &str, time: i64, output: u64, text: &str) -> String {
        let time = chrono::DateTime::from_timestamp(time, 0)
            .unwrap()
            .to_rfc3339();
        json!({
            "type": "assistant",
            "timestamp": time,
            "message": {
                "id": id,
                "model": model,
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": output,
                    "cache_creation_input_tokens": 100,
                    "cache_read_input_tokens": 1000
                },
                "content": [
                    {"type": "text", "text": text},
                    {"type": "tool_use", "name": "mcp__context7__query-docs", "input": {}}
                ]
            }
        })
        .to_string()
    }

    #[test]
    fn counts_each_answer_once_and_separates_sub_agents() {
        // Files are filtered by modification time, which is the real clock's.
        let now = chrono::Utc::now().timestamp();
        let dir = tempfile::tempdir().unwrap();
        let session = dir.path().join(".claude/projects/-work-site");
        fs::create_dir_all(session.join("abc/subagents")).unwrap();
        let main = [
            line("m1", "claude-opus-5-5", now - 60, 300, "first block"),
            line("m1", "claude-opus-5-5", now - 60, 300, "second block"),
            line("m2", "claude-opus-5-5", now - 30 * 3600, 999, "too old"),
            r#"{"type":"user","message":{"content":"hi"}}"#.to_string(),
            line("m3", "<synthetic>", now - 50, 5, "none"),
        ];
        fs::write(session.join("abc.jsonl"), main.join("\n")).unwrap();
        let agent = [line("s1", "claude-haiku-5-5", now - 40, 100, "summary")];
        fs::write(
            session.join("abc/subagents/agent-1.jsonl"),
            agent.join("\n"),
        )
        .unwrap();

        let usage = claude_code(&env_in(dir.path()), now - 5 * 3600);
        assert_eq!(usage.main["claude-opus-5-5"].output, 300);
        assert_eq!(usage.main["claude-opus-5-5"].processed(), 410);
        assert_eq!(usage.main["claude-opus-5-5"].cache_read, 1000);
        assert!(!usage.main.contains_key("<synthetic>"));
        assert_eq!(usage.delegated["claude-haiku-5-5"].output, 100);
        assert_eq!(usage.delegated_share(), Some(25.0));
        assert_eq!(usage.mcp_calls["context7"], 3);
        assert_eq!(usage.estimated, 0, "the counts were plausible");
    }

    #[test]
    fn an_output_count_far_below_the_answer_is_estimated() {
        let now = chrono::Utc::now().timestamp();
        let dir = tempfile::tempdir().unwrap();
        let agent = dir.path().join(".claude/projects/-w/abc/subagents");
        fs::create_dir_all(&agent).unwrap();
        let lines = [
            line("s1", "claude-haiku-5-5", now - 60, 8, &"a".repeat(4000)),
            line("s1", "claude-haiku-5-5", now - 60, 8, &"b".repeat(4000)),
        ];
        fs::write(agent.join("agent-1.jsonl"), lines.join("\n")).unwrap();
        let usage = claude_code(&env_in(dir.path()), now - 3600);
        let output = usage.delegated["claude-haiku-5-5"].output;
        assert!((2000..2100).contains(&output), "{output}");
        assert_eq!(usage.estimated, 1);
    }

    #[test]
    fn nothing_measured_is_not_a_share() {
        assert_eq!(Usage::default().delegated_share(), None);
    }

    #[test]
    fn human_counts() {
        assert_eq!(human(950), "950");
        assert_eq!(human(12_345), "12.3 k");
        assert_eq!(human(4_100_000), "4.1 M");
    }
}
