//! End-to-end tests: run the real `vario` binary against a throwaway home directory.
//! Unix only: the home directory is set through `HOME`, which Windows does not use.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const USER_RULES: &str = "# My rules\n";

struct Output {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// A temporary machine: its own home, data directory and an empty PATH.
struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    /// A home where Claude Code is installed, with the user's own `CLAUDE.md`.
    fn new() -> Self {
        let sandbox = Self::without_client();
        fs::create_dir_all(sandbox.claude_dir()).unwrap();
        fs::write(sandbox.claude("CLAUDE.md"), USER_RULES).unwrap();
        sandbox
    }

    /// A home with no `.claude` folder.
    fn without_client() -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("home")).unwrap();
        fs::create_dir_all(dir.path().join("bin")).unwrap();
        Sandbox { dir }
    }

    fn data_dir(&self) -> PathBuf {
        self.dir.path().join("data")
    }

    fn claude_dir(&self) -> PathBuf {
        self.dir.path().join("home").join(".claude")
    }

    fn claude(&self, file: &str) -> PathBuf {
        self.claude_dir().join(file)
    }

    fn rules(&self) -> String {
        fs::read_to_string(self.claude("CLAUDE.md")).unwrap()
    }

    fn run(&self, args: &[&str]) -> Output {
        let output = Command::new(env!("CARGO_BIN_EXE_vario"))
            .args(args)
            .env_clear()
            .env("HOME", self.dir.path().join("home"))
            .env("VARIO_HOME", self.data_dir())
            .env("PATH", self.dir.path().join("bin"))
            .stdin(Stdio::null())
            .output()
            .unwrap();
        Output {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }
}

fn exists(path: &Path) -> bool {
    path.exists()
}

#[test]
fn install_without_yes_changes_nothing() {
    let sandbox = Sandbox::new();
    let out = sandbox.run(&["install"]);
    assert_eq!(out.code, Some(0), "{}{}", out.stdout, out.stderr);
    assert!(out.stdout.contains("nothing was changed"), "{}", out.stdout);
    assert_eq!(sandbox.rules(), USER_RULES);
    assert!(!exists(&sandbox.data_dir()));
}

#[test]
fn install_dry_run_shows_the_plan_and_writes_nothing() {
    let sandbox = Sandbox::new();
    let out = sandbox.run(&["install", "--dry-run"]);
    assert_eq!(out.code, Some(0), "{}{}", out.stdout, out.stderr);
    assert!(out.stdout.contains("Planned changes"), "{}", out.stdout);
    assert_eq!(sandbox.rules(), USER_RULES);
    assert!(!exists(&sandbox.claude("agents")));
    assert!(!exists(&sandbox.data_dir()));
}

#[test]
fn install_then_doctor_reports_claude_code() {
    let sandbox = Sandbox::new();
    let out = sandbox.run(&["install", "--yes"]);
    assert_eq!(out.code, Some(0), "{}{}", out.stdout, out.stderr);
    assert!(sandbox.claude("agents/vario-reader.md").is_file());
    assert!(sandbox.claude("agents/vario-worker.md").is_file());
    assert!(sandbox.data_dir().join("setup.json").is_file());

    let out = sandbox.run(&["doctor"]);
    assert_eq!(out.code, Some(0), "{}{}", out.stdout, out.stderr);
    assert!(out.stdout.contains("Claude Code"), "{}", out.stdout);
    assert!(out.stdout.contains('✔'), "{}", out.stdout);
}

#[test]
fn remove_needs_yes_and_restores_the_user_content() {
    let sandbox = Sandbox::new();
    assert_eq!(sandbox.run(&["install", "--yes"]).code, Some(0));

    let out = sandbox.run(&["remove"]);
    assert_eq!(out.code, Some(1), "{}{}", out.stdout, out.stderr);
    assert!(sandbox.claude("agents/vario-reader.md").is_file());
    assert!(sandbox.claude("agents/vario-worker.md").is_file());
    assert_ne!(sandbox.rules(), USER_RULES);
    assert!(sandbox.data_dir().join("setup.json").is_file());

    let out = sandbox.run(&["remove", "--yes"]);
    assert_eq!(out.code, Some(0), "{}{}", out.stdout, out.stderr);
    assert_eq!(sandbox.rules(), USER_RULES);
    assert!(!exists(&sandbox.claude("agents/vario-reader.md")));
    assert!(!exists(&sandbox.claude("agents/vario-worker.md")));
    assert!(!exists(&sandbox.data_dir().join("setup.json")));
}

#[test]
fn restore_lists_then_puts_back_the_original_files() {
    let sandbox = Sandbox::new();
    assert_eq!(sandbox.run(&["install", "--yes"]).code, Some(0));

    let out = sandbox.run(&["restore", "--list"]);
    assert_eq!(out.code, Some(0), "{}{}", out.stdout, out.stderr);
    assert!(out.stdout.contains("(3 files)"), "{}", out.stdout);
    assert_eq!(out.stdout.lines().count(), 1, "{}", out.stdout);

    let out = sandbox.run(&["restore", "--yes"]);
    assert_eq!(out.code, Some(0), "{}{}", out.stdout, out.stderr);
    assert_eq!(sandbox.rules(), USER_RULES);
    assert!(!exists(&sandbox.claude("agents/vario-reader.md")));
    assert!(!exists(&sandbox.claude("agents/vario-worker.md")));
}

#[test]
fn an_unknown_client_is_a_usage_error() {
    let sandbox = Sandbox::new();
    let out = sandbox.run(&["install", "--client", "inconnu"]);
    assert_eq!(out.code, Some(2), "{}{}", out.stdout, out.stderr);
    assert!(out.stderr.contains("claude"), "{}", out.stderr);
    assert_eq!(sandbox.rules(), USER_RULES);
}

#[test]
fn a_home_without_a_client_installs_nothing() {
    let sandbox = Sandbox::without_client();
    let out = sandbox.run(&["install", "--yes"]);
    assert_eq!(out.code, Some(0), "{}{}", out.stdout, out.stderr);
    assert!(
        out.stdout.contains("No supported AI client was found"),
        "{}",
        out.stdout
    );
    assert!(!exists(&sandbox.claude_dir()));
}
