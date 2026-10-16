//! Supported AI clients: how to detect each one and what Vario installs in it.
//!
//! Paths and formats come from each client's official documentation.

mod claude_code;
pub mod subagents;

use std::collections::HashMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::parts::Part;
use crate::paths;

/// What detection and paths depend on: the home directory and the environment
/// variables. Tests build their own instead of changing the process environment.
#[derive(Clone, Debug, Default)]
pub struct Env {
    pub home: PathBuf,
    pub vars: HashMap<String, OsString>,
}

impl Env {
    pub fn current() -> Self {
        Env {
            home: paths::home_dir(),
            vars: env::vars_os()
                .map(|(name, value)| (name.to_string_lossy().into_owned(), value))
                .collect(),
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

/// A client Vario can configure.
pub struct Integration {
    /// Identifier used on the command line (`vario install --client claude`).
    pub name: &'static str,
    pub label: &'static str,
    detect: fn(&Env) -> bool,
    parts: fn(&Env) -> Vec<Box<dyn Part>>,
}

impl Integration {
    pub fn detect(&self, env: &Env) -> bool {
        (self.detect)(env)
    }

    pub fn parts(&self, env: &Env) -> Vec<Box<dyn Part>> {
        (self.parts)(env)
    }
}

/// Every supported client, in the order they are shown.
pub fn integrations() -> Vec<Integration> {
    vec![claude_code::integration()]
}

pub fn by_name(name: &str) -> Option<Integration> {
    integrations()
        .into_iter()
        .find(|integration| integration.name == name)
}
