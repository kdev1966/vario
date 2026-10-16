//! Building blocks of an integration. Each part can apply, remove and check one change
//! in a client's configuration.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::error::{Error, Reason, Result};
use crate::files::{self, Action, Session};

/// The key under which Vario registers itself in JSON configurations.
pub const SERVER_NAME: &str = "vario";
/// Whether a hook command runs Vario's `hook` subcommand: `vario hook …`,
/// `/usr/local/bin/vario hook …`, `"C:\Program Files\vario.exe" hook …`.
pub fn is_vario_hook_command(command: &str) -> bool {
    command.match_indices(" hook ").any(|(at, _)| {
        let program = command[..at].trim_end_matches(['"', '\'']);
        let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
        let name = name.to_lowercase();
        name == "vario" || name == "vario.exe"
    })
}

/// What a part brings to a client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Economical sub-agents the client can delegate to.
    Subagents,
    /// Delegation guidance in an instructions file.
    Instructions,
    /// Commands the client runs on its events.
    Hooks,
    /// Vario's MCP server registered with the client.
    Mcp,
}

pub trait Part {
    fn kind(&self) -> Kind;
    fn apply(&self, session: &mut Session) -> Result<()>;
    fn remove(&self, session: &mut Session) -> Result<()>;
    /// Whether the change is in place; any error counts as "no".
    fn present(&self) -> bool;
}

fn key_refs(keys: &[String]) -> Vec<&str> {
    keys.iter().map(String::as_str).collect()
}

/// `data[keys…][name] = value` in a JSON file (an MCP server entry, typically).
pub struct JsonEntry {
    pub path: PathBuf,
    pub keys: Vec<String>,
    pub name: String,
    pub value: Value,
    pub kind: Kind,
}

impl Part for JsonEntry {
    fn kind(&self) -> Kind {
        self.kind
    }

    fn apply(&self, session: &mut Session) -> Result<()> {
        let mut data = files::read_json(&self.path)?;
        let keys = key_refs(&self.keys);
        let container = files::nested_mut(&mut data, &keys, true)
            .ok_or_else(|| Error::config(&self.path, Reason::NotAnObject(keys.join("."))))?;
        container.insert(self.name.clone(), self.value.clone());
        let action = Action::EntrySet(format!("{}.{}", keys.join("."), self.name));
        session.write_text(&self.path, &files::dump_json(&data), action)
    }

    fn remove(&self, session: &mut Session) -> Result<()> {
        if !self.path.exists() {
            return Ok(());
        }
        let mut data = files::read_json(&self.path)?;
        let removed = files::nested_mut(&mut data, &key_refs(&self.keys), false)
            .and_then(|container| container.shift_remove(&self.name))
            .is_some();
        if !removed {
            return Ok(());
        }
        let action = Action::EntryRemoved(self.name.clone());
        session.write_text(&self.path, &files::dump_json(&data), action)
    }

    fn present(&self) -> bool {
        files::read_json(&self.path).is_ok_and(|data| {
            files::nested(&data, &key_refs(&self.keys)).is_some_and(|c| c.contains_key(&self.name))
        })
    }
}

/// Hook entries in a JSON file: `data[keys…][event]` is a list; ours are recognized by
/// their command ([`is_vario_hook_command`]), so the user's own hooks are kept.
pub struct JsonHooks {
    pub path: PathBuf,
    pub keys: Vec<String>,
    /// Event name and the entry Vario adds to its list, in order.
    pub entries: Vec<(String, Value)>,
    /// Top-level keys the file needs, set only when absent (Cursor's `"version": 1`).
    pub defaults: Map<String, Value>,
    pub kind: Kind,
}

/// Whether any string in the entry is a Vario hook command (Claude Code nests the
/// command in `hooks[].command`, Cursor puts it at the top).
fn is_ours(item: &Value) -> bool {
    match item {
        Value::String(text) => is_vario_hook_command(text),
        Value::Array(items) => items.iter().any(is_ours),
        Value::Object(map) => map.values().any(is_ours),
        _ => false,
    }
}

impl JsonHooks {
    fn events(&self) -> String {
        let names: Vec<&str> = self
            .entries
            .iter()
            .map(|(event, _)| event.as_str())
            .collect();
        names.join(", ")
    }
}

impl Part for JsonHooks {
    fn kind(&self) -> Kind {
        self.kind
    }

    fn apply(&self, session: &mut Session) -> Result<()> {
        let mut data = files::read_json(&self.path)?;
        for (key, value) in &self.defaults {
            data.entry(key.clone()).or_insert_with(|| value.clone());
        }
        let keys = key_refs(&self.keys);
        let hooks = files::nested_mut(&mut data, &keys, true)
            .ok_or_else(|| Error::config(&self.path, Reason::NotAnObject(keys.join("."))))?;
        for (event, entry) in &self.entries {
            let mut items = match hooks.get(event) {
                None => Vec::new(),
                Some(Value::Array(items)) => items.clone(),
                Some(_) => {
                    return Err(Error::config(
                        &self.path,
                        Reason::HooksNotAList(event.clone()),
                    ));
                }
            };
            items.retain(|item| !is_ours(item));
            items.push(entry.clone());
            hooks.insert(event.clone(), Value::Array(items));
        }
        let action = Action::HooksSet(self.events());
        session.write_text(&self.path, &files::dump_json(&data), action)
    }

    fn remove(&self, session: &mut Session) -> Result<()> {
        if !self.path.exists() {
            return Ok(());
        }
        let mut data = files::read_json(&self.path)?;
        let Some(hooks) = files::nested_mut(&mut data, &key_refs(&self.keys), false) else {
            return Ok(());
        };
        let mut changed = false;
        for (event, _) in &self.entries {
            let Some(Value::Array(items)) = hooks.get_mut(event) else {
                continue;
            };
            if !items.iter().any(is_ours) {
                continue;
            }
            items.retain(|item| !is_ours(item));
            changed = true;
            if items.is_empty() {
                hooks.shift_remove(event);
            }
        }
        if !changed {
            return Ok(());
        }
        // Drop the hooks container if it is now empty; a file left with nothing but
        // Vario's defaults was Vario's alone, and is deleted.
        if hooks.is_empty()
            && let [key] = self.keys.as_slice()
        {
            data.shift_remove(key);
        }
        if data
            .iter()
            .all(|(key, value)| self.defaults.get(key) == Some(value))
        {
            return session.delete(&self.path, Action::Deleted);
        }
        let action = Action::HooksRemoved(self.events());
        session.write_text(&self.path, &files::dump_json(&data), action)
    }

    fn present(&self) -> bool {
        let Ok(data) = files::read_json(&self.path) else {
            return false;
        };
        let Some(hooks) = files::nested(&data, &key_refs(&self.keys)) else {
            return false;
        };
        // The exact entry: a command left by a moved or older binary counts as missing,
        // so `doctor --fix` rewrites it.
        self.entries.iter().all(|(event, entry)| {
            hooks
                .get(event)
                .and_then(Value::as_array)
                .is_some_and(|items| items.contains(entry))
        })
    }
}

/// The Vario block inside an instructions file shared with the user (`CLAUDE.md`,
/// `AGENTS.md`…); everything outside the block is kept.
pub struct MarkdownBlock {
    pub path: PathBuf,
    pub body: String,
}

impl Part for MarkdownBlock {
    fn kind(&self) -> Kind {
        Kind::Instructions
    }

    fn apply(&self, session: &mut Session) -> Result<()> {
        let current = files::read_text(&self.path)?;
        session.write_text(
            &self.path,
            &files::upsert_block(&current, &self.body),
            Action::InstructionsSet,
        )
    }

    fn remove(&self, session: &mut Session) -> Result<()> {
        let current = files::read_text(&self.path)?;
        if !files::has_block(&current) {
            return Ok(());
        }
        let remaining = files::remove_block(&current);
        if remaining.is_empty() {
            session.delete(&self.path, Action::InstructionsFileDeleted)
        } else {
            session.write_text(&self.path, &remaining, Action::InstructionsRemoved)
        }
    }

    fn present(&self) -> bool {
        files::read_text(&self.path).is_ok_and(|text| files::has_block(&text))
    }
}

/// A file that belongs to Vario alone (a sub-agent definition, typically).
///
/// With a `marker`, a file already at `path` is treated as Vario's only if it contains
/// the marker: a file without it is someone else's and is never overwritten or deleted.
pub struct OwnFile {
    pub path: PathBuf,
    pub content: String,
    pub kind: Kind,
    pub marker: Option<String>,
}

impl OwnFile {
    /// `Ok(false)` when the file exists but does not carry the marker.
    fn owned(&self) -> Result<bool> {
        match &self.marker {
            Some(marker) if self.path.exists() => {
                Ok(files::read_text(&self.path)?.contains(marker.as_str()))
            }
            _ => Ok(true),
        }
    }

    fn foreign(path: &Path) -> Error {
        Error::config(path, Reason::Foreign)
    }
}

impl Part for OwnFile {
    fn kind(&self) -> Kind {
        self.kind
    }

    fn apply(&self, session: &mut Session) -> Result<()> {
        if !self.owned()? {
            return Err(Self::foreign(&self.path));
        }
        session.write_text(&self.path, &self.content, Action::Written)
    }

    fn remove(&self, session: &mut Session) -> Result<()> {
        if !self.path.exists() {
            return Ok(());
        }
        if !self.owned()? {
            return Err(Self::foreign(&self.path));
        }
        session.delete(&self.path, Action::Deleted)
    }

    /// The exact content: a file written by an older Vario, or naming a moved binary,
    /// counts as missing, so `doctor --fix` rewrites it.
    fn present(&self) -> bool {
        files::read_text(&self.path).is_ok_and(|text| text == self.content)
    }
}
