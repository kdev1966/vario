//! Supported AI clients: how to detect each one and what Vario installs in it.
//!
//! Paths and formats come from each client's official documentation.

pub(crate) mod claude_code;
mod codex;
mod cursor;
mod gemini;
mod opencode;
pub mod subagents;
mod vscode;

use std::collections::HashMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::files;
use crate::parts::Part;
use crate::paths;

/// What detection and paths depend on: the home directory and the environment
/// variables. Tests build their own instead of changing the process environment.
#[derive(Clone, Debug, Default)]
pub struct Env {
    pub home: PathBuf,
    pub vars: HashMap<String, OsString>,
    /// The `vario` executable that hook commands run; empty means `vario` on `PATH`.
    pub exe: PathBuf,
}

impl Env {
    pub fn current() -> Self {
        Env {
            home: paths::home_dir(),
            vars: env::vars_os()
                .map(|(name, value)| (name.to_string_lossy().into_owned(), value))
                .collect(),
            exe: env::current_exe().unwrap_or_default(),
        }
    }

    /// The command a client runs for a hook: `<exe> hook <event> --via <client>`. The
    /// absolute path keeps working when `PATH` differs in the client's environment.
    pub fn hook_command(&self, event: &str, via: &str) -> String {
        format!("{} hook {event} --via {via}", self.program())
    }

    /// Claude Code's status line command: `<exe> statusline`.
    pub fn status_line_command(&self) -> String {
        format!("{} statusline", self.program())
    }

    /// The `vario` executable, quoted when its path needs it.
    fn program(&self) -> String {
        let exe = self.exe.to_string_lossy();
        if exe.is_empty() {
            "vario".to_string()
        } else if exe.contains([' ', '"', '\'']) {
            format!("\"{exe}\"")
        } else {
            exe.into_owned()
        }
    }

    /// A variable's value; an empty value counts as unset.
    pub fn var(&self, name: &str) -> Option<&OsStr> {
        self.vars
            .get(name)
            .map(OsString::as_os_str)
            .filter(|value| !value.is_empty())
    }

    /// The executable `name` found on `PATH`, like the shell would.
    pub fn which(&self, name: &str) -> Option<PathBuf> {
        let extensions: Vec<String> = if cfg!(windows) {
            self.var("PATHEXT")
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_else(|| ".EXE;.CMD;.BAT;.COM".into())
                .split(';')
                .filter(|ext| !ext.is_empty())
                .map(str::to_owned)
                .collect()
        } else {
            vec![String::new()]
        };
        env::split_paths(self.var("PATH")?)
            .flat_map(|dir| {
                extensions
                    .iter()
                    .map(move |ext| dir.join(format!("{name}{ext}")))
            })
            .find(|candidate| is_executable(candidate))
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Whether the JSON file registers an MCP server `name` in the object at `key`
/// (`mcpServers` for most clients).
fn has_mcp_server(path: &Path, key: &str, name: &str) -> bool {
    files::read_json(path).is_ok_and(|data| {
        files::nested(&data, &[key]).is_some_and(|servers| servers.contains_key(name))
    })
}

/// A client Vario can configure.
pub struct Integration {
    /// Identifier used on the command line (`vario install --client claude`).
    pub name: &'static str,
    pub label: &'static str,
    detect: fn(&Env) -> bool,
    parts: fn(&Env) -> Vec<Box<dyn Part>>,
    /// Whether Handoff's MCP server is registered with this client.
    handoff_connected: fn(&Env) -> bool,
}

impl Integration {
    pub fn detect(&self, env: &Env) -> bool {
        (self.detect)(env)
    }

    pub fn parts(&self, env: &Env) -> Vec<Box<dyn Part>> {
        (self.parts)(env)
    }

    pub fn handoff_connected(&self, env: &Env) -> bool {
        (self.handoff_connected)(env)
    }
}

/// Every supported client, in the order they are shown.
pub fn integrations() -> Vec<Integration> {
    vec![
        claude_code::integration(),
        cursor::integration(),
        opencode::integration(),
        codex::integration(),
        gemini::integration(),
        vscode::integration(),
    ]
}

pub fn by_name(name: &str) -> Option<Integration> {
    integrations()
        .into_iter()
        .find(|integration| integration.name == name)
}
