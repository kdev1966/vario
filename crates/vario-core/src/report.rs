//! Reading and summarizing the delegation journal that the hooks write.

use std::collections::VecDeque;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::error::{Result, io};
use crate::hooks::{self, JOURNAL, REPEAT_LOOKBACK};

pub fn journal_path(data_dir: &Path) -> PathBuf {
    data_dir.join(JOURNAL)
}

/// One journaled delegation; only the fields the summary needs.
#[derive(Debug, Deserialize)]
struct Delegation {
    time: String,
    agent: String,
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    escalate: Option<bool>,
}

/// Counts for one sub-agent, or for all of them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub delegations: usize,
    /// The sub-agent answered "ESCALATE: yes".
    pub escalated: usize,
    /// The answer had no report footer (no RESULT line).
    pub without_footer: usize,
}

impl Counts {
    fn add(&mut self, delegation: &Delegation) {
        self.delegations += 1;
        self.escalated += usize::from(delegation.escalate == Some(true));
        self.without_footer += usize::from(delegation.result.is_none());
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// Per sub-agent, sorted by name.
    pub agents: Vec<(String, Counts)>,
    pub total: Counts,
    /// Times of the first and last delegations (RFC 3339).
    pub first: Option<String>,
    pub last: Option<String>,
    /// Lines that could not be read, skipped.
    pub unreadable: usize,
}

/// Summarize the journal; a missing journal is an empty summary.
pub fn summarize(data_dir: &Path) -> Result<Summary> {
    let path = journal_path(data_dir);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Summary::default()),
        Err(err) => return Err(io(&path)(err)),
    };
    let mut summary = Summary::default();
    // The last entries counted, to leave out the ones a host reported twice.
    let mut recent: VecDeque<Value> = VecDeque::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            summary.unreadable += 1;
            continue;
        };
        if recent.iter().any(|earlier| hooks::repeats(&entry, earlier)) {
            continue;
        }
        let Ok(delegation) = Delegation::deserialize(&entry) else {
            summary.unreadable += 1;
            continue;
        };
        if recent.len() == REPEAT_LOOKBACK {
            recent.pop_front();
        }
        recent.push_back(entry);
        summary.total.add(&delegation);
        match summary
            .agents
            .iter_mut()
            .find(|(agent, _)| *agent == delegation.agent)
        {
            Some((_, counts)) => counts.add(&delegation),
            None => {
                let mut counts = Counts::default();
                counts.add(&delegation);
                summary.agents.push((delegation.agent.clone(), counts));
            }
        }
        if summary
            .first
            .as_ref()
            .is_none_or(|first| delegation.time < *first)
        {
            summary.first = Some(delegation.time.clone());
        }
        if summary
            .last
            .as_ref()
            .is_none_or(|last| delegation.time > *last)
        {
            summary.last = Some(delegation.time);
        }
    }
    summary.agents.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_journal_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(summarize(dir.path()).unwrap(), Summary::default());
    }

    #[test]
    fn counts_per_sub_agent() {
        let dir = tempfile::tempdir().unwrap();
        let lines = [
            r#"{"time":"2026-10-16T10:00:00+01:00","client":"claude","agent":"vario-worker","result":"ok","escalate":false}"#,
            r#"{"time":"2026-10-15T09:00:00+01:00","client":"cursor","agent":"vario-reader","result":"ok","escalate":false}"#,
            r#"{"time":"2026-10-16T11:00:00+01:00","client":"claude","agent":"vario-worker","result":"stuck","escalate":true}"#,
            r#"{"time":"2026-10-16T12:00:00+01:00","client":"claude","agent":"vario-worker","result":null,"escalate":null}"#,
            // Reported twice by the host: counted once.
            r#"{"time":"2026-10-16T12:00:05+01:00","client":"claude","agent":"vario-worker","result":"stuck","escalate":true}"#,
            r#"{"time":"2026-10-16T12:00:09+01:00","client":"claude","agent":"vario-worker","result":"stuck","escalate":true}"#,
            "not json",
            "",
        ];
        fs::write(journal_path(dir.path()), lines.join("\n")).unwrap();
        let summary = summarize(dir.path()).unwrap();
        let worker = Counts {
            delegations: 4,
            escalated: 2,
            without_footer: 1,
        };
        let reader = Counts {
            delegations: 1,
            ..Counts::default()
        };
        assert_eq!(
            summary.agents,
            [
                ("vario-reader".into(), reader),
                ("vario-worker".into(), worker)
            ]
        );
        assert_eq!(summary.total.delegations, 5);
        assert_eq!(summary.first.as_deref(), Some("2026-10-15T09:00:00+01:00"));
        assert_eq!(summary.last.as_deref(), Some("2026-10-16T12:00:05+01:00"));
        assert_eq!(summary.unreadable, 1);
    }
}
