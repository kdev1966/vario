//! The client the user is working in, and its model and usage, for the gauge.
//!
//! Claude Code exposes a subscription quota (see `quota`); the other clients expose no
//! quota, only their session's model and token/cost usage. They report it through their
//! hooks (`vario hook activity --via <client>`), which writes this file; the gauge shows
//! whichever client was seen most recently.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::Result;
use crate::files;

const FILE: &str = "active.json";

/// One client's current session, as its hook reported it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Active {
    pub client: String,
    pub model: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub cost: f64,
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default)]
    pub reasoning: u64,
    #[serde(default)]
    pub cache_read: u64,
    #[serde(default)]
    pub cache_write: u64,
    #[serde(default)]
    pub directory: String,
    #[serde(default)]
    pub session: String,
    /// Unix seconds when the client reported it.
    pub at: i64,
}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

impl Active {
    /// Read the report a hook sent, leniently: a missing or odd field is simply left at
    /// its default, so a client whose payload changes never breaks the gauge.
    pub fn from_report(client: &str, report: &Map<String, Value>, at: i64) -> Active {
        let text = |key: &str| {
            report
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let number = |key: &str| report.get(key).and_then(Value::as_f64).unwrap_or(0.0);
        let count = |key: &str| number(key).max(0.0) as u64;
        Active {
            client: client.to_string(),
            model: text("model"),
            agent: text("agent"),
            cost: number("cost"),
            input: count("input"),
            output: count("output"),
            reasoning: count("reasoning"),
            cache_read: count("cache_read"),
            cache_write: count("cache_write"),
            directory: text("directory"),
            session: text("session"),
            at,
        }
    }

    pub fn load(data_dir: &Path) -> Option<Active> {
        let text = std::fs::read_to_string(path(data_dir)).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save(&self, data_dir: &Path) -> Result<()> {
        let json = serde_json::to_string(self).expect("an active report serializes");
        files::atomic_write(&path(data_dir), &(json + "\n"))
    }

    /// The label shown in the gauge: the model, else the client.
    pub fn label(&self) -> &str {
        if self.model.is_empty() {
            &self.client
        } else {
            &self.model
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_report_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Active::load(dir.path()), None);
        let report = json!({
            "model": "deepseek-v4.1-flash-free",
            "agent": "build",
            "cost": 0.0,
            "input": 1_081_226,
            "output": 32_777,
            "reasoning": 10_341,
            "cache_read": 3_509_632,
            "cache_write": 0,
            "directory": "/work",
            "session": "ses_1"
        })
        .as_object()
        .unwrap()
        .clone();
        let active = Active::from_report("opencode", &report, 1_800_000_000);
        active.save(dir.path()).unwrap();
        assert_eq!(Active::load(dir.path()), Some(active.clone()));
        assert_eq!(active.output, 32_777);
        assert_eq!(active.client, "opencode");
        assert_eq!(active.label(), "deepseek-v4.1-flash-free");
    }

    #[test]
    fn odd_fields_are_left_at_their_default() {
        let report = json!({"model": 42, "output": "many"})
            .as_object()
            .unwrap()
            .clone();
        let active = Active::from_report("codex", &report, 10);
        assert_eq!(active.model, "", "a non-string model is ignored");
        assert_eq!(active.output, 0);
        assert_eq!(active.label(), "codex");
    }
}
