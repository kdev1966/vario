//! Location of Vario's own data, following each platform's conventions.

use std::env;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

/// Overrides the data directory, for tests and unusual setups.
pub const ENV_HOME: &str = "VARIO_HOME";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Windows,
    MacOs,
    /// Linux and the other Unix systems, which follow the XDG conventions.
    Other,
}

impl Os {
    pub fn current() -> Self {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Other
        }
    }
}

/// The directory that holds Vario's backups and state.
pub fn data_dir() -> PathBuf {
    data_dir_for(Os::current(), |name| env::var_os(name), &home_dir())
}

/// `VARIO_HOME` overrides the default, which is:
/// - Windows: `%LOCALAPPDATA%\vario`
/// - macOS: `~/Library/Application Support/vario`
/// - other systems: `$XDG_DATA_HOME/vario`, or `~/.local/share/vario`
pub fn data_dir_for(os: Os, var: impl Fn(&str) -> Option<OsString>, home: &Path) -> PathBuf {
    let set = |name: &str| {
        var(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    if let Some(dir) = set(ENV_HOME) {
        return expand_home(&dir, home);
    }
    match os {
        Os::Windows => set("LOCALAPPDATA")
            .unwrap_or_else(|| home.join("AppData").join("Local"))
            .join("vario"),
        Os::MacOs => home
            .join("Library")
            .join("Application Support")
            .join("vario"),
        Os::Other => set("XDG_DATA_HOME")
            .unwrap_or_else(|| home.join(".local").join("share"))
            .join("vario"),
    }
}

/// The user's home directory; the current directory if none can be determined.
pub fn home_dir() -> PathBuf {
    env::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// Replace a leading `~` with the home directory.
fn expand_home(path: &Path, home: &Path) -> PathBuf {
    let mut components = path.components();
    match components.next() {
        Some(Component::Normal(first)) if first == "~" => home.join(components.as_path()),
        _ => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn override_wins_and_expands_home() {
        let home = Path::new("/home/ana");
        let dir = data_dir_for(Os::Other, env_of(&[(ENV_HOME, "~/vario-test")]), home);
        assert_eq!(dir, Path::new("/home/ana/vario-test"));
    }

    #[test]
    fn platform_defaults() {
        let home = Path::new("/home/ana");
        let none = env_of(&[]);
        assert_eq!(
            data_dir_for(Os::MacOs, &none, home),
            Path::new("/home/ana/Library/Application Support/vario")
        );
        assert_eq!(
            data_dir_for(Os::Other, &none, home),
            Path::new("/home/ana/.local/share/vario")
        );
        assert_eq!(
            data_dir_for(Os::Windows, &none, home),
            home.join("AppData").join("Local").join("vario")
        );
    }

    #[test]
    fn platform_variables_are_used_unless_empty() {
        let home = Path::new("/home/ana");
        assert_eq!(
            data_dir_for(Os::Other, env_of(&[("XDG_DATA_HOME", "/data")]), home),
            Path::new("/data/vario")
        );
        assert_eq!(
            data_dir_for(Os::Other, env_of(&[("XDG_DATA_HOME", "")]), home),
            Path::new("/home/ana/.local/share/vario")
        );
        assert_eq!(
            data_dir_for(Os::Windows, env_of(&[("LOCALAPPDATA", "/appdata")]), home),
            Path::new("/appdata").join("vario")
        );
    }
}
