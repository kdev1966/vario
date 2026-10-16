//! What the user chose during setup, remembered in `<data>/setup.json`, so that
//! `vario doctor --fix` repairs those clients and does not ask again about the others.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::clients::{Env, integrations};
use crate::error::{Result, io};
use crate::files;
use crate::install;

const FILE: &str = "setup.json";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Choices {
    /// Clients Vario is installed in, sorted.
    pub clients: Vec<String>,
    /// Clients the user chose not to configure, sorted; never in `clients`.
    pub declined: Vec<String>,
    /// Vario has already suggested installing Handoff; it does not suggest it again.
    pub handoff_suggested: bool,
}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

impl Choices {
    /// The saved choices; `None` when there are none, or when the file is unreadable
    /// (it is then rewritten at the next setup).
    pub fn load(data_dir: &Path) -> Option<Choices> {
        let text = fs::read_to_string(path(data_dir)).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// The saved choices, plus every client where Vario is found installed: a setup
    /// done by hand or by an older version still counts as chosen.
    pub fn effective(data_dir: &Path, env: &Env) -> Option<Choices> {
        let saved = Choices::load(data_dir);
        let configured: Vec<String> = integrations()
            .iter()
            .filter(|integration| install::installed(integration, env))
            .map(|integration| integration.name.to_string())
            .collect();
        if saved.is_none() && configured.is_empty() {
            return None;
        }
        let mut choices = saved.unwrap_or_default();
        choices.choose(&configured);
        Some(choices)
    }

    /// Record clients as chosen; they leave the declined list.
    pub fn choose(&mut self, names: &[String]) {
        self.clients.extend(names.iter().cloned());
        self.normalize();
    }

    /// Record clients as declined, unless they are chosen.
    pub fn decline(&mut self, names: &[String]) {
        self.declined.extend(names.iter().cloned());
        self.normalize();
    }

    pub fn is_settled(&self, name: &str) -> bool {
        self.clients.iter().chain(&self.declined).any(|n| n == name)
    }

    fn normalize(&mut self) {
        self.clients.sort();
        self.clients.dedup();
        self.declined.sort();
        self.declined.dedup();
        let clients = &self.clients;
        self.declined.retain(|name| !clients.contains(name));
    }

    /// Add to the saved choices: clients chosen and declined in this run.
    pub fn remember(data_dir: &Path, chosen: &[String], declined: &[String]) -> Result<()> {
        let mut choices = Choices::load(data_dir).unwrap_or_default();
        choices.choose(chosen);
        choices.decline(declined);
        choices.save(data_dir)
    }

    pub fn save(&self, data_dir: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self).expect("choices always serialize");
        files::atomic_write(&path(data_dir), &(json + "\n"))
    }

    pub fn delete(data_dir: &Path) -> Result<()> {
        let file = path(data_dir);
        match fs::remove_file(&file) {
            Err(err) if err.kind() != ErrorKind::NotFound => Err(io(&file)(err)),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn choosing_a_declined_client_moves_it() {
        let mut choices = Choices::default();
        choices.decline(&names(&["cursor", "opencode"]));
        choices.choose(&names(&["cursor", "claude", "claude"]));
        assert_eq!(choices.clients, names(&["claude", "cursor"]));
        assert_eq!(choices.declined, names(&["opencode"]));
        assert!(choices.is_settled("opencode") && !choices.is_settled("codex"));
    }

    #[test]
    fn save_then_load_and_tolerate_missing_keys() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Choices::load(dir.path()), None);
        let mut choices = Choices::default();
        choices.choose(&names(&["claude"]));
        choices.save(dir.path()).unwrap();
        assert_eq!(Choices::load(dir.path()), Some(choices));

        fs::write(path(dir.path()), r#"{"clients": ["claude"]}"#).unwrap();
        let old = Choices::load(dir.path()).unwrap();
        assert!(old.declined.is_empty() && !old.handoff_suggested);

        fs::write(path(dir.path()), "not json").unwrap();
        assert_eq!(Choices::load(dir.path()), None);
        Choices::delete(dir.path()).unwrap();
        Choices::delete(dir.path()).unwrap();
        assert!(!path(dir.path()).exists());
    }
}
